use std::collections::{HashMap, HashSet};

fn is_third_party_group(group: &crate::models::quota::QuotaGroup) -> bool {
    let name = group.display_name.to_ascii_lowercase();
    name.contains("claude")
        || name.contains("gpt")
        || name.contains("3p")
        || group
            .buckets
            .iter()
            .any(|bucket| bucket.bucket_id.to_ascii_lowercase().contains("3p"))
}

fn group_matches_model(group: &crate::models::quota::QuotaGroup, model: &str) -> bool {
    let third_party = is_third_party_group(group);
    if model.starts_with("claude") || model.starts_with("gpt") {
        third_party
    } else {
        model.starts_with("gemini") && !third_party
    }
}

fn is_weekly_bucket(bucket: &crate::models::quota::QuotaBucket) -> bool {
    let key = format!("{} {}", bucket.window, bucket.bucket_id).to_ascii_lowercase();
    key.contains("week") || key.contains("7d")
}

/// 所有匹配额度窗口都构成上限，包括周额度和五小时额度。
pub fn limiting_bucket<'a>(
    model: &str,
    groups: &'a [crate::models::quota::QuotaGroup],
) -> Option<&'a crate::models::quota::QuotaBucket> {
    let model = model.to_ascii_lowercase();
    groups
        .iter()
        .filter(|group| group_matches_model(group, &model))
        .flat_map(|group| &group.buckets)
        .filter(|bucket| {
            bucket.remaining_fraction.is_finite()
                && (0.0..=1.0).contains(&bucket.remaining_fraction)
        })
        .min_by(|a, b| a.remaining_fraction.total_cmp(&b.remaining_fraction))
}

/// 返回目标模型组最新快照中额度最低的原始周额度桶。
pub fn weekly_limiting_bucket<'a>(
    model: &str,
    groups: &'a [crate::models::quota::QuotaGroup],
) -> Option<&'a crate::models::quota::QuotaBucket> {
    let model = model.to_ascii_lowercase();
    groups
        .iter()
        .filter(|group| group_matches_model(group, &model))
        .flat_map(|group| &group.buckets)
        .filter(|bucket| {
            is_weekly_bucket(bucket)
                && bucket.remaining_fraction.is_finite()
                && (0.0..=1.0).contains(&bucket.remaining_fraction)
        })
        .min_by(|a, b| a.remaining_fraction.total_cmp(&b.remaining_fraction))
}

/// 返回目标模型组最新快照中的原始周额度比例，不混入 5 小时或模型额度。
pub fn weekly_remaining_fraction(
    model: &str,
    groups: &[crate::models::quota::QuotaGroup],
) -> Option<f64> {
    weekly_limiting_bucket(model, groups).map(|bucket| bucket.remaining_fraction)
}

/// 根据原始周额度快照更新保护键。缺失或无效的周额度不是恢复证据，因此保留旧状态。
pub fn reconcile_weekly_protection(
    existing: &HashSet<String>,
    quota: &crate::models::quota::QuotaData,
    monitored_models: &[String],
    threshold_percentage: u32,
) -> HashSet<String> {
    let monitored: HashSet<String> = monitored_models.iter().cloned().collect();
    let mut protected: HashSet<String> = existing.intersection(&monitored).cloned().collect();
    let Some(groups) = quota.quota_groups.as_deref() else {
        return protected;
    };
    let threshold = f64::from(threshold_percentage.min(100)) / 100.0;
    for model in monitored {
        let Some(remaining) = weekly_remaining_fraction(&model, groups) else {
            continue;
        };
        if remaining <= threshold {
            protected.insert(model);
        } else {
            protected.remove(&model);
        }
    }
    protected
}

/// 将供应商的多个窗口合并为模型可使用额度的上限。
pub fn constrain_data(quota: &mut crate::models::quota::QuotaData) {
    let Some(groups) = quota.quota_groups.as_ref() else {
        return;
    };
    for model in &mut quota.models {
        if let Some(bucket) = limiting_bucket(&model.name, groups) {
            let percentage = (bucket.remaining_fraction.min(1.0) * 100.0).floor() as i32;
            if percentage <= model.percentage {
                model.percentage = percentage;
                if !bucket.reset_time.is_empty() {
                    model.reset_time = bucket.reset_time.clone();
                }
            }
        }
    }
}

/// 在加载旧快照时补齐窗口约束，只更新模型百分比和对应重置时间。
pub fn constrain_snapshot(account: &mut serde_json::Value) {
    let Some(quota) = account.get_mut("quota") else {
        return;
    };
    let Some(groups) = quota.get("quota_groups").cloned() else {
        return;
    };
    let Ok(groups) = serde_json::from_value::<Vec<crate::models::quota::QuotaGroup>>(groups) else {
        return;
    };
    let Some(models) = quota
        .get_mut("models")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for model in models {
        let name = model
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if let Some(bucket) = limiting_bucket(name, &groups) {
            let percentage = (bucket.remaining_fraction.min(1.0) * 100.0).floor() as i64;
            if percentage
                <= model
                    .get("percentage")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(100)
            {
                model["percentage"] = percentage.into();
                if !bucket.reset_time.is_empty() {
                    model["reset_time"] = bucket.reset_time.clone().into();
                }
            }
        }
    }
}

/// 优先采用实际模型的额度；旧记录缺少该型号时采用所属额度组的保守值。
pub fn model_percentage(
    exact: &HashMap<String, i32>,
    groups: &HashMap<String, i32>,
    model: &str,
    group: &str,
) -> Option<i32> {
    exact
        .get(&model.to_ascii_lowercase())
        .or_else(|| groups.get(group))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_quota_fresh_data_respects_both_windows_and_model_cap() {
        for fractions in [[0.1, 0.9], [0.9, 0.1]] {
            let mut quota: crate::models::quota::QuotaData = serde_json::from_value(serde_json::json!({
                "last_updated": 1,
                "models": [
                    {"name":"gemini-3.8-flash-high","percentage":100,"reset_time":"model-reset"},
                    {"name":"gemini-3.7-flash-high","percentage":5,"reset_time":"model-reset"}
                ],
                "quota_groups": [{"display_name":"Gemini Models","buckets":[
                    {"bucket_id":"gemini-weekly","window":"weekly","remaining_fraction":fractions[0],"reset_time":"weekly-reset"},
                    {"bucket_id":"gemini-5h","window":"5h","remaining_fraction":fractions[1],"reset_time":"short-reset"}
                ]}]
            })).unwrap();
            constrain_data(&mut quota);
            assert_eq!(quota.models[0].percentage, 10);
            assert_eq!(quota.models[1].percentage, 5);
            assert_eq!(quota.models[1].reset_time, "model-reset");
        }
    }

    #[test]
    fn compat_quota_weekly_window_cannot_be_hidden_by_five_hour_balance() {
        let mut account = serde_json::json!({"quota": {
            "models": [{"name":"gemini-3.8-flash-high","percentage":100,"extra":"keep"}, {"name":"claude-sonnet-4-6","percentage":80}],
            "quota_groups": [{"display_name":"Gemini Models","buckets":[
                {"bucket_id":"gemini-5h","window":"5h","remaining_fraction":0.9,"reset_time":"soon"},
                {"bucket_id":"gemini-weekly","window":"weekly","remaining_fraction":0.08,"reset_time":"later"}
            ]}]
        }});
        constrain_snapshot(&mut account);
        assert_eq!(account["quota"]["models"][0]["percentage"], 8);
        assert_eq!(account["quota"]["models"][0]["reset_time"], "later");
        assert_eq!(account["quota"]["models"][0]["extra"], "keep");
        assert_eq!(account["quota"]["models"][1]["percentage"], 80);
    }

    #[test]
    fn weekly_reserve_uses_raw_weekly_fraction_only() {
        let quota = |five_hour, weekly| {
            serde_json::from_value::<crate::models::quota::QuotaData>(serde_json::json!({
                "last_updated": 1,
                "models": [{"name":"gemini-3.8-flash-high","percentage":5,"reset_time":"short"}],
                "quota_groups": [{"display_name":"Gemini Models","buckets":[
                    {"bucket_id":"gemini-5h","window":"5h","remaining_fraction":five_hour,"reset_time":"short"},
                    {"bucket_id":"gemini-weekly","window":"weekly","remaining_fraction":weekly,"reset_time":"weekly"}
                ]}]
            }))
            .unwrap()
        };
        let monitored = vec!["gemini-3-flash".to_string()];
        let empty = HashSet::new();

        assert!(
            reconcile_weekly_protection(&empty, &quota(0.05, 0.10001), &monitored, 10).is_empty()
        );
        assert!(
            reconcile_weekly_protection(&empty, &quota(1.0, 0.10), &monitored, 10)
                .contains("gemini-3-flash")
        );
    }

    #[test]
    fn weekly_reserve_requires_fresh_recovery_evidence() {
        let monitored = vec!["claude".to_string()];
        let existing = HashSet::from(["claude".to_string()]);
        let missing: crate::models::quota::QuotaData = serde_json::from_value(serde_json::json!({
            "last_updated": 2,
            "models": [{"name":"claude-sonnet-4-6","percentage":100,"reset_time":""}]
        }))
        .unwrap();
        assert_eq!(
            reconcile_weekly_protection(&existing, &missing, &monitored, 10),
            existing
        );

        let recovered: crate::models::quota::QuotaData = serde_json::from_value(serde_json::json!({
            "last_updated": 3,
            "models": [{"name":"claude-sonnet-4-6","percentage":100,"reset_time":""}],
            "quota_groups": [{"display_name":"Claude and GPT models","buckets":[
                {"bucket_id":"3p-weekly","window":"7d","remaining_fraction":0.11,"reset_time":"next"}
            ]}]
        }))
        .unwrap();
        assert!(reconcile_weekly_protection(&existing, &recovered, &monitored, 10).is_empty());
    }
    #[test]
    fn compat_quota_other_model_cannot_hide_low_target() {
        let exact = HashMap::from([
            ("gemini-3.8-flash-high".to_string(), 10),
            ("gemini-3.7-flash-high".to_string(), 100),
        ]);
        let groups = HashMap::from([("gemini-3-flash".to_string(), 100)]);
        assert_eq!(
            model_percentage(&exact, &groups, "gemini-3.8-flash-high", "gemini-3-flash"),
            Some(10)
        );
        assert_eq!(
            model_percentage(&exact, &groups, "gemini-3.7-flash-high", "gemini-3-flash"),
            Some(100)
        );
    }
}
