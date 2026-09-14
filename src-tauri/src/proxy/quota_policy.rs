use std::collections::HashMap;

/// 所有匹配额度窗口都构成上限，包括周额度和五小时额度。
pub fn limiting_bucket<'a>(
    model: &str,
    groups: &'a [crate::models::quota::QuotaGroup],
) -> Option<&'a crate::models::quota::QuotaBucket> {
    let model = model.to_ascii_lowercase();
    groups
        .iter()
        .filter(|group| {
            let name = group.display_name.to_ascii_lowercase();
            let third_party =
                name.contains("claude") || name.contains("gpt") || name.contains("3p");
            if model.starts_with("claude") || model.starts_with("gpt") {
                third_party
            } else {
                model.starts_with("gemini") && (name.contains("gemini") || !third_party)
            }
        })
        .flat_map(|group| &group.buckets)
        .filter(|bucket| bucket.remaining_fraction.is_finite() && bucket.remaining_fraction >= 0.0)
        .min_by(|a, b| a.remaining_fraction.total_cmp(&b.remaining_fraction))
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

/// 保护边界包含阈值本身；未知额度不由百分比规则直接判为耗尽。
pub fn is_protected(percentage: Option<i32>, threshold: i32) -> bool {
    percentage.is_some_and(|remaining| remaining <= threshold)
}

/// 将独立的 reasoning_effort 转回有档位的 Gemini Flash 型号候选。
/// 调用方负责确认候选确实存在于账号提供的型号列表中。
pub fn effort_model_candidate(model: &str, effort: Option<&str>) -> Option<String> {
    let version = model.strip_prefix("gemini-")?.strip_suffix("-flash")?;
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let effort = effort?;
    if !matches!(effort, "low" | "medium" | "high") {
        return None;
    }
    Some(format!("{model}-{effort}"))
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
    fn compat_quota_boundary_includes_ten_percent() {
        assert!(is_protected(Some(0), 10));
        assert!(is_protected(Some(9), 10));
        assert!(is_protected(Some(10), 10));
        assert!(!is_protected(Some(11), 10));
        assert!(!is_protected(None, 10));
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
    #[test]
    fn compat_effort_restores_stripped_suffix_without_changing_explicit_model() {
        assert_eq!(
            effort_model_candidate("gemini-3.8-flash", Some("high")).as_deref(),
            Some("gemini-3.8-flash-high")
        );
        for name in [
            "gemini-3.8-flash-high",
            "gemini-3.8-flash-tiered",
            "claude-sonnet-4-6",
            "gpt-4o",
        ] {
            assert!(effort_model_candidate(name, Some("high")).is_none());
        }
        assert!(effort_model_candidate("gemini-3.8-flash", None).is_none());
    }
}
