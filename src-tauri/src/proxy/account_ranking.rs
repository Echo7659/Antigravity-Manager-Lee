use std::cmp::Ordering;

/// 单次账号快照的排序信息；账号 ID 用于相同优先级下的确定性排序。
#[derive(Clone, Copy)]
pub struct AccountRank<'a> {
    pub tier: Option<&'a str>,
    pub quota: i32,
    pub health: f32,
    pub reset_time: Option<i64>,
    pub account_id: &'a str,
}

pub(crate) fn tier_priority(tier: Option<&str>) -> u8 {
    let tier = tier.unwrap_or_default().to_ascii_lowercase();
    if tier.contains("ultra") {
        0
    } else if tier.contains("pro") {
        1
    } else if tier.contains("free") {
        2
    } else {
        3
    }
}

fn health_score(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        f32::NEG_INFINITY
    }
}

/// 返回已按等级排序的候选列表中最高等级的账号数，不限制为固定大小的池。
pub fn highest_tier_pool_len<'a>(tiers: impl IntoIterator<Item = Option<&'a str>>) -> usize {
    let mut priorities = tiers.into_iter().map(tier_priority);
    let Some(first) = priorities.next() else {
        return 0;
    };
    1 + priorities.take_while(|tier| *tier == first).count()
}

/// 按等级、配额、健康度、重置时间分组和账号 ID 比较账号，保证全序。
pub fn compare_accounts(a: AccountRank<'_>, b: AccountRank<'_>) -> Ordering {
    tier_priority(a.tier)
        .cmp(&tier_priority(b.tier))
        .then_with(|| b.quota.cmp(&a.quota))
        .then_with(|| health_score(b.health).total_cmp(&health_score(a.health)))
        // 固定分组保持十分钟粒度，避免两两时间差产生不具传递性的相等关系。
        .then_with(|| {
            a.reset_time
                .unwrap_or(i64::MAX)
                .div_euclid(600)
                .cmp(&b.reset_time.unwrap_or(i64::MAX).div_euclid(600))
        })
        .then_with(|| a.account_id.cmp(b.account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_ranking_samples_entire_best_tier_pool() {
        let tiers = [Some("PRO"); 20];
        assert_eq!(highest_tier_pool_len(tiers), 20);
        assert_eq!(
            highest_tier_pool_len([Some("ULTRA"), Some("PRO"), Some("FREE")]),
            1
        );
        assert_eq!(highest_tier_pool_len([]), 0);
    }

    fn rank(reset_time: Option<i64>) -> AccountRank<'static> {
        AccountRank {
            tier: Some("PRO"),
            quota: 100,
            health: 1.0,
            reset_time,
            account_id: "a",
        }
    }

    #[test]
    fn compat_ranking_is_transitive_across_reset_boundaries() {
        let times = [
            Some(i64::MIN),
            Some(-1),
            Some(0),
            Some(599),
            Some(600),
            Some(900),
            Some(1199),
            Some(1200),
            Some(i64::MAX),
            None,
        ];
        for a in times {
            for b in times {
                for c in times {
                    let ab = compare_accounts(rank(a), rank(b));
                    let bc = compare_accounts(rank(b), rank(c));
                    assert_eq!(ab, compare_accounts(rank(b), rank(a)).reverse());
                    if ab != Ordering::Greater && bc != Ordering::Greater {
                        assert_ne!(compare_accounts(rank(a), rank(c)), Ordering::Greater);
                    }
                }
            }
        }
    }

    #[test]
    fn compat_ranking_handles_nonfinite_health_and_extreme_times() {
        let health = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            0.0,
            0.5,
            1.0,
        ];
        let mut ranks = Vec::new();
        for i in 0..500 {
            let mut r = rank(Some(if i % 2 == 0 { i64::MIN } else { i64::MAX }));
            r.health = health[i % health.len()];
            ranks.push(r);
        }
        ranks.sort_by(|a, b| compare_accounts(*a, *b));
        for pair in ranks.windows(2) {
            assert_ne!(compare_accounts(pair[0], pair[1]), Ordering::Greater);
        }
    }

    #[test]
    fn compat_ranking_preserves_priority_and_stable_ties() {
        let a = rank(None);
        let mut b = a;
        b.tier = Some("ultra");
        assert_eq!(compare_accounts(a, b), Ordering::Greater);
        b = a;
        b.quota = 50;
        assert_eq!(compare_accounts(a, b), Ordering::Less);
        b = a;
        b.health = 0.5;
        assert_eq!(compare_accounts(a, b), Ordering::Less);
        b = a;
        b.reset_time = Some(0);
        assert_eq!(compare_accounts(a, b), Ordering::Greater);
        b = a;
        b.account_id = "b";
        assert_eq!(compare_accounts(a, b), Ordering::Less);
    }
}
