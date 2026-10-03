pub mod account;
pub mod account_service;
pub mod config;
pub mod device;
pub mod i18n;
pub mod log_bridge;
pub mod logger;
pub mod migration;
pub mod oauth;
pub mod oauth_server;
pub mod proxy_db;
pub mod quota;
pub mod quota_refresh;
pub mod scheduler;
pub mod security_db;
pub mod token_stats;
pub mod user_token;
pub mod user_token_db;

use crate::models;

// Re-export commonly used functions to the top level of the modules namespace for easy external calling
pub use account::*;
#[allow(unused_imports)]
pub use logger::*;
#[allow(unused_imports)]
pub use quota::*;
// pub use device::*;

pub async fn fetch_quota(
    access_token: &str,
    email: &str,
    account_id: Option<&str>,
) -> crate::error::AppResult<(models::QuotaData, Option<String>)> {
    quota::fetch_quota(access_token, email, account_id).await
}
