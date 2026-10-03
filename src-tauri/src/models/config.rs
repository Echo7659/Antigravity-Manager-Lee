use crate::proxy::ProxyConfig;
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub language: String,
    pub theme: String,
    pub auto_refresh: bool,
    pub refresh_interval: i32, // minutes
    pub auto_sync: bool,
    pub sync_interval: i32, // minutes
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub scheduled_warmup: ScheduledWarmupConfig, // [NEW] Scheduled warmup configuration
    #[serde(default)]
    pub quota_protection: QuotaProtectionConfig, // [NEW] Quota protection configuration
    #[serde(default)]
    pub pinned_quota_models: PinnedQuotaModelsConfig, // [NEW] Pinned quota models list
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig, // [NEW] Circuit breaker configuration
    #[serde(default)]
    pub hidden_menu_items: Vec<String>, // Hidden menu item path list
    #[serde(default)]
    pub suggestion_delete_thinking_store: Option<bool>, // [NEW] 建议删除历史思考块缓存开关
    #[serde(default)]
    pub thinking_cleanup_dismissed: Option<bool>, // [NEW] 用户是否已确认/忽略该建议
    #[serde(default)]
    pub dismissed_thinking_cleanup_version: Option<String>, // [NEW] 用户已确认或忽略建议的目标版本号
}

/// Scheduled warmup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledWarmupConfig {
    /// Whether smart warmup is enabled
    pub enabled: bool,

    /// List of models to warmup
    #[serde(default = "default_warmup_models")]
    pub monitored_models: Vec<String>,
}

fn default_warmup_models() -> Vec<String> {
    vec![
        "gemini-3-flash".to_string(),
        "claude".to_string(),
        "gemini-3-pro-high".to_string(),
        "gemini-3.1-flash-image".to_string(),
    ]
}

impl ScheduledWarmupConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            monitored_models: default_warmup_models(),
        }
    }
}

impl Default for ScheduledWarmupConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Quota protection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaProtectionConfig {
    /// Whether quota protection is enabled
    pub enabled: bool,

    /// Reserved weekly quota percentage (1-99)
    pub threshold_percentage: u32,

    /// List of monitored models (e.g. gemini-3-flash, gemini-3-pro-high, gemini-3.1-pro-high, claude-sonnet-4-6)
    #[serde(default = "default_monitored_models")]
    pub monitored_models: Vec<String>,
}

fn default_monitored_models() -> Vec<String> {
    vec![
        "claude".to_string(),
        "gemini-3-pro-high".to_string(),
        "gemini-3-flash".to_string(),
        "gemini-3.1-flash-image".to_string(),
    ]
}

impl QuotaProtectionConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            threshold_percentage: 10, // Default 10% reserve
            monitored_models: default_monitored_models(),
        }
    }
}

impl Default for QuotaProtectionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Pinned quota models configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedQuotaModelsConfig {
    /// List of pinned models (displayed outside the account list)
    #[serde(default = "default_pinned_models")]
    pub models: Vec<String>,
}

fn default_pinned_models() -> Vec<String> {
    vec![
        "gemini-3-pro-high".to_string(),
        "gemini-3-flash".to_string(),
        "gemini-3.1-flash-image".to_string(),
        "claude-sonnet-4-6-thinking".to_string(),
    ]
}

impl PinnedQuotaModelsConfig {
    pub fn new() -> Self {
        Self {
            models: default_pinned_models(),
        }
    }
}

impl Default for PinnedQuotaModelsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Whether circuit breaker is enabled
    pub enabled: bool,

    /// Unified backoff steps (seconds)
    /// Default: [60, 300, 1800, 7200]
    #[serde(default = "default_backoff_steps")]
    pub backoff_steps: Vec<u64>,

    /// Optional 5h zero-quota lock; exhausted weekly quota always blocks scheduling.
    #[serde(default = "default_lock_on_zero_quota")]
    pub lock_on_zero_quota: bool,
}

fn default_backoff_steps() -> Vec<u64> {
    vec![60, 300, 1800, 7200]
}

fn default_lock_on_zero_quota() -> bool {
    false
}

impl CircuitBreakerConfig {
    pub fn new() -> Self {
        Self {
            enabled: true,
            backoff_steps: default_backoff_steps(),
            lock_on_zero_quota: false,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AppConfig {
    pub fn new() -> Self {
        Self {
            language: crate::modules::i18n::default_language(),
            theme: "system".to_string(),
            auto_refresh: true,
            refresh_interval: 15,
            auto_sync: false,
            sync_interval: 5,
            proxy: ProxyConfig::default(),
            scheduled_warmup: ScheduledWarmupConfig::default(),
            quota_protection: QuotaProtectionConfig::default(),
            pinned_quota_models: PinnedQuotaModelsConfig::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            hidden_menu_items: Vec::new(),
            suggestion_delete_thinking_store: None,
            thinking_cleanup_dismissed: None,
            dismissed_thinking_cleanup_version: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn legacy_desktop_fields_are_ignored() {
        let _dir = crate::proxy::monitor::prompt_log_tests::TestDataDir::new();
        let legacy: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/legacy-desktop-config.json"
        ))
        .unwrap();
        let mut config = AppConfig::new();
        config.proxy.api_key = "legacy-config-key".into();
        config.proxy.custom_mapping.clear();
        config
            .proxy
            .custom_mapping
            .insert("gemini-3.x-flash".into(), "3.x-flash-tiered".into());
        config
            .proxy
            .custom_mapping
            .insert("custom-model".into(), "gemini-3.8-flash".into());
        config.proxy.thinking_budget.flash_high_legacy_migrated = true;
        let mut value = serde_json::to_value(config).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(legacy.as_object().unwrap().clone());
        let original = format!("  {}\n\n", serde_json::to_string_pretty(&value).unwrap());
        let (_, migrated) = crate::modules::config::parse_and_migrate_config(&original).unwrap();
        assert!(!migrated, "fixture must already satisfy current migrations");
        let path = crate::modules::account::get_data_dir()
            .unwrap()
            .join("gui_config.json");
        std::fs::write(&path, original.as_bytes()).unwrap();

        let config = crate::modules::config::load_app_config().unwrap();
        assert_eq!(config.language, "zh");
        assert_eq!(config.proxy.api_key, "legacy-config-key");
        assert_eq!(
            config.proxy.custom_mapping.get("custom-model").unwrap(),
            "gemini-3.8-flash"
        );
        assert_eq!(std::fs::read(path).unwrap(), original.as_bytes());
        let saved = serde_json::to_value(config).unwrap();
        assert!(saved.get("auto_launch").is_none());
    }

    #[test]
    fn saved_language_is_preserved_when_loading_config() {
        let mut config = AppConfig::new();
        for language in ["en", "zh", "zh-TW", "ru"] {
            config.language = language.to_string();
            let saved = serde_json::to_string(&config).unwrap();
            let restored: AppConfig = serde_json::from_str(&saved).unwrap();
            assert_eq!(restored.language, language);
        }
    }
}
