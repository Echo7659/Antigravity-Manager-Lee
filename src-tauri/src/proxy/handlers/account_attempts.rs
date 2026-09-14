use std::collections::HashSet;

/// 单次生成请求最多使用六个不同账号，包含首次请求。
pub const MAX_ACCOUNT_ATTEMPTS: usize = 6;

/// 单次请求的账号排除名单；后续选取不得复用已尝试的账号。
#[derive(Debug, Default)]
pub struct AccountAttempts {
    accounts: HashSet<String>,
}

impl AccountAttempts {
    pub fn record(&mut self, account_id: &str) {
        self.accounts.insert(account_id.to_owned());
    }

    pub fn excluded(&self) -> &HashSet<String> {
        &self.accounts
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.accounts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(pool_size: usize, success_at: Option<usize>) -> (Vec<String>, bool) {
        let pool: Vec<_> = (0..pool_size).map(|i| format!("account-{i}")).collect();
        let mut attempts = AccountAttempts::default();
        let mut sends = Vec::new();
        for _ in 0..MAX_ACCOUNT_ATTEMPTS {
            let Some(account) = pool.iter().find(|id| !attempts.excluded().contains(*id)) else {
                break;
            };
            attempts.record(account);
            sends.push(account.clone());
            if success_at == Some(sends.len()) {
                return (sends, true);
            }
        }
        (sends, false)
    }

    #[test]
    fn compat_failover_uses_initial_plus_five_distinct_accounts() {
        let (sends, success) = drive(20, None);
        assert!(!success);
        assert_eq!(sends.len(), 6);
        assert_eq!(sends.iter().collect::<HashSet<_>>().len(), 6);
    }

    #[test]
    fn compat_failover_stops_at_success_including_sixth_account() {
        for success_at in 1..=6 {
            let (sends, success) = drive(20, Some(success_at));
            assert!(success);
            assert_eq!(sends.len(), success_at);
        }
        assert!(!drive(20, Some(7)).1);
    }

    #[test]
    fn compat_failover_exhausts_small_pool_without_reuse() {
        for pool_size in 0..6 {
            let (sends, success) = drive(pool_size, None);
            assert!(!success);
            assert_eq!(sends.len(), pool_size);
        }
    }

    #[test]
    fn compat_failover_exclusions_are_request_local() {
        let mut first = AccountAttempts::default();
        first.record("account-1");
        assert!(first.excluded().contains("account-1"));
        assert!(AccountAttempts::default().is_empty());
    }
}
