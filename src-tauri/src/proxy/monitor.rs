use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

// Admission happens before spawn_blocking, so queued tasks cannot retain unlimited bodies.
static LOG_WRITERS: Semaphore = Semaphore::const_new(4);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequestLog {
    pub id: String,
    pub timestamp: i64,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration: u64,                // ms
    pub model: Option<String>,        // 客户端请求的模型名
    pub mapped_model: Option<String>, // 实际路由后使用的模型名
    pub account_email: Option<String>,
    pub client_ip: Option<String>, // 客户端 IP 地址
    pub error: Option<String>,
    pub request_body: Option<String>,
    pub upstream_request_body: Option<String>, // 网关转出给上游(Antigravity)的报文
    pub response_body: Option<String>,
    #[serde(default)]
    pub request_headers: Option<String>,
    #[serde(default)]
    pub upstream_request_headers: Option<String>,
    #[serde(default)]
    pub response_headers: Option<String>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cached_tokens: Option<u32>,
    pub protocol: Option<String>, // 协议类型: "openai", "anthropic", "gemini"
    pub username: Option<String>, // User token username
    #[serde(default)]
    pub session_id: Option<String>, // 会话标识 (如 sess-xxx)
}

#[cfg(test)]
pub(crate) mod prompt_log_tests {
    use super::*;

    pub(crate) fn sample_log(id: &str, bytes: usize) -> ProxyRequestLog {
        serde_json::from_value(serde_json::json!({
            "id": id, "timestamp": chrono::Utc::now().timestamp_millis(),
            "method": "POST", "url": "/v1/chat/completions", "status": 500, "duration": 10,
            "request_body": "q".repeat(bytes), "response_body": "错".repeat(bytes),
            "error": "错".repeat(2048)
        }))
        .unwrap()
    }

    // Tests using ABV_DATA_DIR run serially and restore the previous process setting.
    pub(crate) struct TestDataDir {
        _dir: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl TestDataDir {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("ABV_DATA_DIR");
            unsafe { std::env::set_var("ABV_DATA_DIR", dir.path()) };
            Self {
                _dir: dir,
                previous,
            }
        }
    }
    impl Drop for TestDataDir {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                unsafe { std::env::set_var("ABV_DATA_DIR", previous) };
            } else {
                unsafe { std::env::remove_var("ABV_DATA_DIR") };
            }
        }
    }

    #[tokio::test]
    async fn opus_5_5_protocol_stats_survive_disabled_detail_logging() {
        let _dir = TestDataDir::new();
        crate::modules::proxy_db::init_db().unwrap();
        crate::modules::token_stats::init_db().unwrap();
        let monitor = ProxyMonitor {
            logs: RwLock::new(VecDeque::new()),
            stats: RwLock::new(ProxyStats::default()),
            max_logs: 2,
            enabled: Arc::new(AtomicBool::new(false)),
            capture_health_logs: Arc::new(AtomicBool::new(false)),
        };
        let mut log = sample_log("opus-disabled", 100);
        log.status = 200;
        log.model = Some("claude-opus-5-5".into());
        log.mapped_model = Some(crate::proxy::common::model_mapping::resolve_model_route(
            "client-opus",
            &std::collections::HashMap::from([(
                "client-opus".into(),
                "models/anthropic/claude-opus-5.5".into(),
            )]),
        ));
        log.account_email = Some("opus@example.test".into());
        log.input_tokens = Some(1_500);
        log.output_tokens = Some(250);
        log.cached_tokens = Some(500);
        monitor.log_request(log).await;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let totals = crate::modules::token_stats::get_hourly_stats(1).unwrap();
                if totals.iter().any(|row| row.request_count == 1) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("token writer did not finish");
        let conn = rusqlite::Connection::open(crate::modules::token_stats::get_db_path().unwrap())
            .unwrap();
        let billing_model: String = conn
            .query_row("SELECT billing_model FROM token_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            billing_model, "claude-opus-5-5-high",
            "billing uses the resolved default physical tier"
        );
        let models = crate::modules::token_stats::get_model_stats(1).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "claude-opus-5-5");
        assert_eq!(models[0].total_tokens, 1_750);
        assert_eq!(models[0].total_cached_tokens, 500);
        let accounts = crate::modules::token_stats::get_account_stats(1).unwrap();
        assert_eq!(accounts.len(), 1);
        assert!((accounts[0].total_cost_usd - 0.0091).abs() < 1e-12);
        assert_eq!(accounts[0].unpriced_tokens, 0);
        assert!(monitor.logs.read().await.is_empty());
        assert!(crate::modules::proxy_db::get_log_detail("opus-disabled").is_err());
    }

    #[tokio::test]
    async fn prompt_log_memory_summary_and_database_detail() {
        let _dir = TestDataDir::new();
        crate::modules::proxy_db::init_db().unwrap();
        let monitor = ProxyMonitor {
            logs: RwLock::new(VecDeque::new()),
            stats: RwLock::new(ProxyStats::default()),
            max_logs: 2,
            enabled: Arc::new(AtomicBool::new(true)),
            capture_health_logs: Arc::new(AtomicBool::new(false)),
        };
        let log = sample_log("detail", 4096);
        let response = log.response_body.clone();
        monitor.log_request(log).await;
        let _finished = LOG_WRITERS.acquire_many(4).await.unwrap();
        let logs = monitor.logs.read().await;
        assert!(logs[0].request_body.is_none() && logs[0].response_body.is_none());
        assert_eq!(logs[0].error.as_ref().unwrap().chars().count(), 1024);
        let detail = crate::modules::proxy_db::get_log_detail("detail").unwrap();
        assert_eq!(detail.response_body, response);
        assert_eq!(
            detail.request_body.as_deref(),
            Some("q".repeat(4096).as_str())
        );
        monitor.set_enabled(false);
        monitor.log_request(sample_log("disabled", 4096)).await;
        assert!(crate::modules::proxy_db::get_log_detail("disabled").is_err());
    }
}

impl ProxyRequestLog {
    pub(crate) fn summary(&self) -> Self {
        Self {
            id: self.id.clone(),
            timestamp: self.timestamp,
            method: self.method.clone(),
            url: self.url.clone(),
            status: self.status,
            duration: self.duration,
            model: self.model.clone(),
            mapped_model: self.mapped_model.clone(),
            account_email: self.account_email.clone(),
            client_ip: self.client_ip.clone(),
            error: self
                .error
                .as_ref()
                .map(|error| error.chars().take(1024).collect()),
            request_body: None,
            upstream_request_body: None,
            response_body: None,
            request_headers: None,
            upstream_request_headers: None,
            response_headers: None,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_tokens: self.cached_tokens,
            protocol: self.protocol.clone(),
            username: self.username.clone(),
            session_id: self.session_id.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct UpstreamCapture {
    body: Option<String>,
    headers: Option<String>,
}

#[derive(Clone, Default)]
pub struct UpstreamRequestBodyHolder(pub std::sync::Arc<std::sync::Mutex<UpstreamCapture>>);

tokio::task_local! {
    pub static CURRENT_UPSTREAM_CAPTURE: UpstreamRequestBodyHolder;
}

impl UpstreamRequestBodyHolder {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(
            UpstreamCapture::default(),
        )))
    }

    pub fn set(&self, body: String) {
        if let Ok(mut lock) = self.0.lock() {
            lock.body = Some(body);
        }
    }

    pub fn set_headers_json(&self, headers: String) {
        if let Ok(mut lock) = self.0.lock() {
            lock.headers = Some(headers);
        }
    }

    pub fn set_value(&self, val: &serde_json::Value) {
        let clean = sanitize_upstream_debug_value(val);
        if let Ok(s) = serde_json::to_string(&clean) {
            self.set(s);
        }
    }

    pub fn take(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut g| g.body.take())
    }

    pub fn take_headers(&self) -> Option<String> {
        self.0.lock().ok().and_then(|mut g| g.headers.take())
    }
}

fn sanitize_upstream_debug_value(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::String(s)
            if s.starts_with("data:image/") || s.starts_with("data:audio/") =>
        {
            serde_json::Value::String(format!("[inline data omitted: {} chars]", s.len()))
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sanitize_upstream_debug_value).collect())
        }
        serde_json::Value::Object(map) => {
            let is_inline = map
                .get("mimeType")
                .and_then(serde_json::Value::as_str)
                .is_some()
                && map
                    .get("data")
                    .and_then(serde_json::Value::as_str)
                    .is_some();
            serde_json::Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        let v = if is_inline && k == "data" {
                            serde_json::Value::String(format!(
                                "[inline data omitted: {} chars]",
                                v.as_str().map(str::len).unwrap_or_default()
                            ))
                        } else {
                            sanitize_upstream_debug_value(v)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            )
        }
        _ => val.clone(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub success_count: u64,
    pub error_count: u64,
}

pub struct ProxyMonitor {
    pub logs: RwLock<VecDeque<ProxyRequestLog>>,
    pub stats: RwLock<ProxyStats>,
    pub max_logs: usize,
    pub enabled: Arc<AtomicBool>,
    pub capture_health_logs: Arc<AtomicBool>,
}

/// 两类维护独立计时；截止点从完成时刻计算，错过的周期不补跑。
struct MaintenanceSchedule {
    thinking: std::time::Instant,
    logs: std::time::Instant,
}

impl MaintenanceSchedule {
    fn new(now: std::time::Instant) -> Self {
        Self {
            thinking: now,
            logs: now,
        }
    }

    fn next(&self) -> std::time::Instant {
        self.thinking.min(self.logs)
    }

    fn due(&self, now: std::time::Instant) -> (bool, bool) {
        let logs_due = now >= self.logs;
        (now >= self.thinking || logs_due, logs_due)
    }

    fn completed(&mut self, now: std::time::Instant, thinking_retry: Option<bool>, logs_ran: bool) {
        if let Some(retry) = thinking_retry {
            self.thinking = now + std::time::Duration::from_secs(if retry { 60 } else { 3600 });
        }
        if logs_ran {
            self.logs = now + std::time::Duration::from_secs(3600);
        }
    }
}

/// 两种日志保留策略来自同一只读快照；读取失败时沿用各自默认值。
fn load_maintenance_retention() -> (crate::proxy::config::LogRetentionConfig, u64) {
    crate::modules::config::load_app_config_read_only()
        .map(|config| {
            (
                config.proxy.log_retention,
                config.proxy.internal_error_log_retention.budget_bytes(),
            )
        })
        // 0 由内部错误日志的 setter 解释为默认预算。
        .unwrap_or_else(|_| (Default::default(), 0))
}

#[cfg(test)]
mod maintenance_schedule_tests {
    use super::MaintenanceSchedule;
    use std::time::{Duration, Instant};

    #[test]
    fn monitor_retention_policies_share_one_readonly_snapshot() {
        let _dir = super::prompt_log_tests::TestDataDir::new();
        let path = crate::modules::account::resolve_data_dir_read_only()
            .unwrap()
            .join("gui_config.json");
        let mut config = crate::models::AppConfig::new();
        config.proxy.log_retention.max_rows = 4321;
        config.proxy.internal_error_log_retention.max_storage_mb = 1234;
        let expected_budget = config.proxy.internal_error_log_retention.budget_bytes();
        let bytes = serde_json::to_vec(&config).unwrap();
        std::fs::write(&path, &bytes).unwrap();

        let snapshot = super::load_maintenance_retention();
        assert!(std::fs::read(&path).unwrap() == bytes);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(snapshot.0.max_rows, 4321);
        assert_eq!(snapshot.1, expected_budget);

        let fallback = super::load_maintenance_retention();
        assert_eq!(
            fallback.0.max_rows,
            crate::proxy::config::LogRetentionConfig::default().max_rows
        );
        assert_eq!(fallback.1, 0);
        assert!(!path.exists());
    }

    #[test]
    fn monitor_thinking_retries_do_not_accelerate_hourly_log_retention() {
        let now = Instant::now();
        let mut schedule = MaintenanceSchedule::new(now);
        assert_eq!(schedule.due(now), (true, true));
        let completed = now + Duration::from_secs(3);
        schedule.completed(completed, Some(true), true);
        assert_eq!(schedule.next(), completed + Duration::from_secs(60));
        let retry = schedule.next();
        assert_eq!(schedule.due(retry), (true, false));
        // 未完成、延期和错误均保持 60 秒；日志截止点保持不变。
        schedule.completed(retry, Some(true), false);
        assert_eq!(schedule.next(), retry + Duration::from_secs(60));
        let finished = schedule.next();
        schedule.completed(finished, Some(false), false);
        let logs = completed + Duration::from_secs(3600);
        assert_eq!(schedule.next(), logs);
        assert_eq!(schedule.due(logs), (true, true));
        schedule.completed(logs, Some(false), true);
        assert_eq!(schedule.next(), logs + Duration::from_secs(3600));
    }
}

impl ProxyMonitor {
    pub fn new(max_logs: usize) -> Self {
        // Initialize DB
        if let Err(e) = crate::modules::proxy_db::init_db() {
            tracing::error!("Failed to initialize proxy DB: {}", e);
        }

        tokio::spawn(async {
            let mut schedule = MaintenanceSchedule::new(std::time::Instant::now());
            let mut thinking_state = crate::modules::proxy_db::ThinkingMaintenance::default();
            loop {
                tokio::time::sleep_until(schedule.next().into()).await;
                let (thinking_due, logs_due) = schedule.due(std::time::Instant::now());
                let mut state = std::mem::take(&mut thinking_state);
                let result = tokio::task::spawn_blocking(move || {
                    let retention = logs_due.then(load_maintenance_retention);
                    let retention_res = retention
                        .as_ref()
                        .map(|(proxy, _)| crate::modules::proxy_db::apply_retention(proxy));
                    let thinking_res = thinking_due.then(|| {
                        if logs_due {
                            state.reopen_completed_categories();
                        }
                        let days = crate::proxy::config::get_thinking_retention_days() as i64;
                        crate::modules::proxy_db::cleanup_thinking_storage(days, &mut state)
                    });
                    let error_log_res = retention.as_ref().map(|(_, error_budget)| {
                        crate::modules::logger::set_internal_error_log_budget_bytes(*error_budget);
                        crate::modules::logger::apply_internal_error_log_retention()
                    });
                    (state, retention_res, thinking_res, error_log_res)
                })
                .await;
                let mut thinking_retry = true;
                match result {
                    Ok((state, retention_res, thinking_res, error_log_res)) => {
                        thinking_state = state;
                        if let Some(result) = retention_res {
                            match result {
                                Ok((cleared, deleted)) => {
                                    if cleared > 0 || deleted > 0 {
                                        tracing::info!("Proxy log retention: cleared {} bodies, deleted {} rows", cleared, deleted);
                                    }
                                }
                                Err(error) => tracing::error!(
                                    "Failed to apply proxy log retention: {}",
                                    error
                                ),
                            }
                        }
                        if let Some(result) = thinking_res {
                            match result {
                                Ok(stats) => {
                                    thinking_retry = stats.deferred || stats.unfinished;
                                    tracing::info!(
                                        deleted_records = stats.deleted_records,
                                        deleted_sessions = stats.deleted_sessions,
                                        deleted_tools = stats.deleted_tools,
                                        scanned = stats.scanned,
                                        batches = stats.batches,
                                        committed_batches = stats.committed_batches,
                                        deferred = stats.deferred,
                                        defer_reason = ?stats.defer_reason,
                                        unfinished = stats.unfinished,
                                        orphan_cursor = ?stats.orphan_cursor,
                                        orphan_sweep_complete = stats.orphan_sweep_complete,
                                        units = ?stats.units,
                                        elapsed_ms = stats.elapsed.as_millis(),
                                        "Thinking storage maintenance completed"
                                    );
                                }
                                Err(error) => {
                                    tracing::error!("Failed to cleanup thinking records: {}", error)
                                }
                            }
                        }
                        if let Some(Err(error)) = error_log_res {
                            tracing::error!(
                                "Failed to apply internal error log retention: {}",
                                error
                            );
                        }
                    }
                    Err(error) => tracing::error!("Proxy maintenance task failed: {}", error),
                }
                schedule.completed(
                    std::time::Instant::now(),
                    thinking_due.then_some(thinking_retry),
                    logs_due,
                );
            }
        });

        Self {
            logs: RwLock::new(VecDeque::with_capacity(max_logs)),
            stats: RwLock::new(ProxyStats::default()),
            max_logs,
            enabled: Arc::new(AtomicBool::new(false)), // Default to disabled
            capture_health_logs: Arc::new(AtomicBool::new(false)), // Default to false
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_capture_health_logs(&self, enabled: bool) {
        self.capture_health_logs.store(enabled, Ordering::Relaxed);
    }

    pub fn is_capture_health_logs(&self) -> bool {
        self.capture_health_logs.load(Ordering::Relaxed)
    }

    pub async fn log_request(&self, log: ProxyRequestLog) {
        if let (Some(account), Some(input), Some(output)) =
            (&log.account_email, log.input_tokens, log.output_tokens)
        {
            let model = log.model.clone().unwrap_or_else(|| "unknown".to_string());
            let billing_model = log.mapped_model.clone();
            let account = account.clone();
            let cached = log.cached_tokens.unwrap_or(0);
            tokio::task::spawn_blocking(move || {
                if let Err(e) = crate::modules::token_stats::record_usage(
                    &account,
                    &model,
                    billing_model.as_deref(),
                    input,
                    output,
                    cached,
                ) {
                    tracing::debug!("Failed to record token stats: {}", e);
                }
            });
        }

        if !self.is_enabled() {
            return;
        }
        tracing::info!("[Monitor] Logging request: {} {}", log.method, log.url);
        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_requests += 1;
            if log.status >= 200 && log.status < 400 {
                stats.success_count += 1;
            } else {
                stats.error_count += 1;
            }
        }

        let summary = log.summary();
        // Add only the same summary used by the frontend to memory.
        {
            let mut logs = self.logs.write().await;
            if logs.len() >= self.max_logs {
                logs.pop_back();
            }
            logs.push_front(summary.clone());
        }

        let Ok(permit) = LOG_WRITERS.try_acquire() else {
            tracing::debug!("Skipping proxy log persistence: writers busy");
            return;
        };
        // Save to DB (respect server-side simple/full storage mode)
        let mut log_to_save = log;
        let storage_mode = crate::proxy::config::get_payload_storage_mode();
        log_to_save.request_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.request_body,
            &storage_mode,
        );
        log_to_save.upstream_request_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.upstream_request_body,
            &storage_mode,
        );
        log_to_save.response_body = crate::proxy::payload_audit::apply_storage_mode_to_body(
            log_to_save.response_body,
            &storage_mode,
        );

        let enabled = Arc::clone(&self.enabled);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if !enabled.load(Ordering::Relaxed) {
                return;
            }
            if let Err(e) = crate::modules::proxy_db::save_log(log_to_save) {
                tracing::error!("Failed to save proxy log to DB: {}", e);
            }

            // Sync to Security DB (IpAccessLogs) so it appears in Security Monitor
            if let Some(ip) = &summary.client_ip {
                let security_log = crate::modules::security_db::IpAccessLog {
                    id: uuid::Uuid::new_v4().to_string(),
                    client_ip: ip.clone(),
                    timestamp: summary.timestamp / 1000, // ms to s
                    method: Some(summary.method.clone()),
                    path: Some(summary.url.clone()),
                    user_agent: None, // We don't have UA in ProxyRequestLog easily accessible here without plumbing
                    status: Some(summary.status as i32),
                    duration: Some(summary.duration as i64),
                    api_key_hash: None,
                    blocked: false, // This comes from monitor, so it wasn't blocked by IP filter
                    block_reason: None,
                    username: summary.username.clone(),
                };

                if let Err(e) = crate::modules::security_db::save_ip_access_log(&security_log) {
                    tracing::error!("Failed to save security log: {}", e);
                }
            }
        });
    }

    pub async fn get_logs(&self, limit: usize) -> Vec<ProxyRequestLog> {
        // Try to get from DB first for true history
        let db_result =
            tokio::task::spawn_blocking(move || crate::modules::proxy_db::get_logs(limit)).await;

        match db_result {
            Ok(Ok(logs)) => logs,
            Ok(Err(e)) => {
                tracing::error!("Failed to get logs from DB: {}", e);
                // Fallback to memory
                let logs = self.logs.read().await;
                logs.iter().take(limit).cloned().collect()
            }
            Err(e) => {
                tracing::error!("Spawn blocking failed for get_logs: {}", e);
                let logs = self.logs.read().await;
                logs.iter().take(limit).cloned().collect()
            }
        }
    }

    pub async fn get_stats(&self) -> ProxyStats {
        let db_result = tokio::task::spawn_blocking(|| crate::modules::proxy_db::get_stats()).await;

        match db_result {
            Ok(Ok(stats)) => stats,
            Ok(Err(e)) => {
                tracing::error!("Failed to get stats from DB: {}", e);
                self.stats.read().await.clone()
            }
            Err(e) => {
                tracing::error!("Spawn blocking failed for get_stats: {}", e);
                self.stats.read().await.clone()
            }
        }
    }

    pub async fn get_logs_filtered(
        &self,
        page: usize,
        page_size: usize,
        search_text: Option<String>,
        level: Option<String>,
    ) -> Result<Vec<ProxyRequestLog>, String> {
        let offset = (page.max(1) - 1) * page_size;
        let errors_only = level.as_deref() == Some("error");
        let search = search_text.unwrap_or_default();

        let res = tokio::task::spawn_blocking(move || {
            crate::modules::proxy_db::get_logs_filtered(&search, errors_only, page_size, offset)
        })
        .await;

        match res {
            Ok(r) => r,
            Err(e) => Err(format!("Spawn blocking failed: {}", e)),
        }
    }

    pub async fn clear(&self) {
        let mut logs = self.logs.write().await;
        logs.clear();
        let mut stats = self.stats.write().await;
        *stats = ProxyStats::default();

        let _ = tokio::task::spawn_blocking(|| {
            if let Err(e) = crate::modules::proxy_db::clear_logs() {
                tracing::error!("Failed to clear logs in DB: {}", e);
            }
        })
        .await;
    }
}
