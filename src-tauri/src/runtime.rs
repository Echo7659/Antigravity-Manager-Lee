use crate::{models::AppConfig, modules, proxy};
use std::{sync::Arc, time::Duration};

/// 常驻 HTTP 服务及其监听任务；逻辑代理开关不销毁账号池。
pub struct ServerRuntime {
    pub server: proxy::AxumServer,
    task: tokio::task::JoinHandle<()>,
}

impl ServerRuntime {
    pub async fn start(config: AppConfig) -> Result<Self, String> {
        let proxy_config = &config.proxy;
        proxy::update_thinking_budget_config(proxy_config.thinking_budget.clone());
        proxy::update_global_system_prompt_config(proxy_config.global_system_prompt.clone());
        proxy::update_image_thinking_mode(proxy_config.image_thinking_mode.clone());
        proxy::update_multimodal_config(proxy_config.multimodal.clone());
        proxy::config::update_global_audit_config(
            proxy_config.experimental.payload_storage_mode.clone(),
            proxy_config.experimental.log_retention_days,
            proxy_config.experimental.thinking_store_enabled,
            proxy_config.experimental.thinking_retention_days,
            Some(proxy_config.experimental.thinking_max_memory_turns),
        );
        let data_dir = modules::account::get_data_dir()?;
        std::fs::create_dir_all(data_dir.join("accounts")).map_err(|e| e.to_string())?;
        let token_manager = Arc::new(proxy::TokenManager::new(data_dir));
        token_manager
            .update_sticky_config(proxy_config.scheduling.clone())
            .await;
        token_manager
            .update_circuit_breaker_config(config.circuit_breaker)
            .await;
        token_manager.load_accounts().await?;
        token_manager
            .set_preferred_account(proxy_config.preferred_account_id.clone())
            .await;
        let monitor = Arc::new(proxy::monitor::ProxyMonitor::new(1000));
        monitor.set_enabled(proxy_config.enable_logging);
        monitor.set_capture_health_logs(proxy_config.capture_health_logs);
        let (server, task) = proxy::AxumServer::start(
            proxy_config.get_bind_address().to_string(),
            proxy_config.port,
            token_manager.clone(),
            proxy_config.custom_mapping.clone(),
            proxy_config.request_timeout,
            proxy_config.upstream_proxy.clone(),
            proxy_config.user_agent_override.clone(),
            proxy::ProxySecurityConfig::from_proxy_config(proxy_config),
            monitor,
            proxy_config.experimental.clone(),
            proxy_config.debug_logging.clone(),
            Arc::new(crate::commands::cloudflared::CloudflaredState::new()),
            proxy_config.proxy_pool.clone(),
            proxy_config.only_raw_quota_models,
            proxy_config.image_scheduler.clone(),
        )
        .await?;
        token_manager.start_auto_cleanup().await;
        server
            .set_running(proxy_config.auto_start && token_manager.len() > 0)
            .await;
        Ok(Self { server, task })
    }

    pub async fn shutdown(mut self) {
        self.server.set_running(false).await;
        self.server.stop();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.server.cloudflared_state.stop())
            .await;
        self.server
            .token_manager
            .graceful_shutdown(Duration::from_secs(2))
            .await;
        if tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = self.task.await;
        }
    }
}

/// 应用显式启动覆盖；未设置的字段保留配置文件值。
fn apply_startup_overrides(
    config: &mut AppConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Result<bool, String> {
    let mut changed = false;
    if let Some(key) = env("ABV_API_KEY")
        .or_else(|| env("API_KEY"))
        .filter(|v| !v.trim().is_empty())
    {
        changed |= config.proxy.api_key != key;
        config.proxy.api_key = key;
    }
    if let Some(password) = env("ABV_WEB_PASSWORD")
        .or_else(|| env("WEB_PASSWORD"))
        .filter(|v| !v.trim().is_empty())
    {
        changed |= config.proxy.admin_password.as_ref() != Some(&password);
        config.proxy.admin_password = Some(password);
    }
    if let Some(value) = env("ABV_AUTH_MODE").or_else(|| env("AUTH_MODE")) {
        let mode = match value.trim().to_ascii_lowercase().as_str() {
            "off" => proxy::ProxyAuthMode::Off,
            "auto" => proxy::ProxyAuthMode::Auto,
            "strict" => proxy::ProxyAuthMode::Strict,
            "all_except_health" => proxy::ProxyAuthMode::AllExceptHealth,
            _ => return Err("Invalid ABV_AUTH_MODE/AUTH_MODE".into()),
        };
        changed |= std::mem::discriminant(&config.proxy.auth_mode) != std::mem::discriminant(&mode);
        config.proxy.auth_mode = mode;
    }
    if let Some(value) = env("ABV_BIND_LOCAL_ONLY") {
        let local_only = match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => return Err("Invalid ABV_BIND_LOCAL_ONLY".into()),
        };
        changed |= config.proxy.allow_lan_access == local_only;
        config.proxy.allow_lan_access = !local_only;
    }
    Ok(changed)
}

async fn probe_health(port: u16) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let probe = async {
        let mut stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = [0_u8; 256];
        let read = stream.read(&mut response).await?;
        Ok::<_, std::io::Error>(
            response[..read].starts_with(b"HTTP/1.1 200 ")
                || response[..read].starts_with(b"HTTP/1.0 200 "),
        )
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(5), probe).await,
        Ok(Ok(true))
    )
}

async fn select_shutdown_signal(
    interrupt: impl std::future::Future<Output = Result<(), String>>,
    terminate: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    tokio::select! {
        result = interrupt => result,
        result = terminate => result,
    }
}

async fn shutdown_signal() -> Result<(), String> {
    let interrupt = async { tokio::signal::ctrl_c().await.map_err(|e| e.to_string()) };
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| e.to_string())?;
        select_shutdown_signal(interrupt, async {
            terminate
                .recv()
                .await
                .ok_or_else(|| "SIGTERM signal stream closed".to_string())
        })
        .await
    }
    #[cfg(not(unix))]
    {
        select_shutdown_signal(interrupt, std::future::pending()).await
    }
}

/// 退出事件到达后结束显式持有的任务，并关闭 HTTP 监听和账号池任务。
async fn shutdown_after_signal(
    runtime: ServerRuntime,
    scheduler: tokio::task::JoinHandle<()>,
    watchdog: tokio::task::JoinHandle<()>,
    signal: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let result = signal.await;
    scheduler.abort();
    let _ = scheduler.await;
    watchdog.abort();
    let _ = watchdog.await;
    runtime.shutdown().await;
    result
}

pub async fn run() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--health-check") {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8045);
        return if probe_health(port).await {
            Ok(())
        } else {
            Err("HTTP health check failed".into())
        };
    }
    modules::logger::init_logger();
    modules::token_stats::init_db()?;
    modules::security_db::init_db()?;
    modules::user_token_db::init_db()?;
    let mut config = modules::config::load_app_config()?;
    if apply_startup_overrides(&mut config, |key| std::env::var(key).ok())? {
        modules::config::save_app_config(&config)?;
    }
    let port = config.proxy.port;
    let runtime = ServerRuntime::start(config).await?;
    let scheduler = modules::scheduler::start_scheduler(runtime.server.token_manager.clone());
    let watchdog_enabled = std::env::var("ABV_HEALTH_WATCHDOG_ENABLED")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(true);
    let watchdog = tokio::spawn(async move {
        if !watchdog_enabled {
            return;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut failures = 0;
        loop {
            failures = if probe_health(port).await {
                0
            } else {
                failures + 1
            };
            if failures >= 3 {
                tracing::error!("HTTP runtime is unresponsive; exiting for supervisor restart");
                std::process::exit(1);
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
    shutdown_after_signal(runtime, scheduler, watchdog, shutdown_signal()).await
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::models::{Account, AppConfig, QuotaData, TokenData};
    use std::{fs, sync::Arc};

    pub(crate) struct ServerFixture {
        pub runtime: super::ServerRuntime,
        pub config: AppConfig,
        pub client: reqwest::Client,
        pub url: String,
        dir: tempfile::TempDir,
        previous_dir: Option<std::ffi::OsString>,
    }

    impl ServerFixture {
        pub async fn new() -> Self {
            Self::with_account(true).await
        }

        async fn with_account(include_account: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let previous_dir = std::env::var_os("ABV_DATA_DIR");
            unsafe { std::env::set_var("ABV_DATA_DIR", dir.path()) };
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            let mut config = AppConfig::new();
            config.proxy.port = port;
            config.proxy.auto_start = true;
            config.proxy.allow_lan_access = false;
            config.proxy.api_key = "fixture-key".into();
            config.proxy.admin_password = Some("fixture-key".into());
            config.proxy.only_raw_quota_models = true;
            config.proxy.custom_mapping.clear();
            crate::modules::config::save_app_config(&config).unwrap();
            let mut account = Account::new(
                "fixture-account".into(),
                "fixture@example.test".into(),
                TokenData::new(
                    "access".into(),
                    "refresh".into(),
                    3600,
                    Some("fixture@example.test".into()),
                    Some("fixture-project".into()),
                    None,
                    false,
                    None,
                ),
            );
            account.quota = Some(
                serde_json::from_value::<QuotaData>(serde_json::json!({
                    "last_updated": chrono::Utc::now().timestamp(),
                    "models": [{"name":"gemini-3.8-flash", "percentage":100, "reset_time":""}]
                }))
                .unwrap(),
            );
            if include_account {
                fs::create_dir_all(dir.path().join("accounts")).unwrap();
                crate::modules::account::save_account(&account).unwrap();
            }
            let runtime = super::ServerRuntime::start(config.clone()).await.unwrap();
            Self {
                runtime,
                config,
                client: reqwest::Client::builder()
                    .no_proxy()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .unwrap(),
                url: format!("http://127.0.0.1:{port}"),
                dir,
                previous_dir,
            }
        }

        pub async fn get_model_ids(&self) -> Vec<String> {
            self.client
                .get(format!("{}/api/proxy/models", self.url))
                .bearer_auth("fixture-key")
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap()
        }

        pub fn snapshot_data_files(&self) -> Vec<Vec<u8>> {
            [
                "gui_config.json",
                "accounts.json",
                "accounts/fixture-account.json",
            ]
            .iter()
            .map(|path| fs::read(self.dir.path().join(path)).unwrap())
            .collect()
        }

        pub async fn stop_proxy(&self) {
            self.client
                .post(format!("{}/api/proxy/stop", self.url))
                .bearer_auth("fixture-key")
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }

        pub async fn save_config(&self, config: &AppConfig) {
            self.client
                .post(format!("{}/api/config", self.url))
                .bearer_auth("fixture-key")
                .json(&serde_json::json!({"config":config}))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap();
        }

        pub async fn shutdown(self) {
            self.runtime.shutdown().await;
            unsafe {
                match self.previous_dir {
                    Some(value) => std::env::set_var("ABV_DATA_DIR", value),
                    None => std::env::remove_var("ABV_DATA_DIR"),
                }
            }
        }
    }

    #[tokio::test]
    async fn server_runtime_keeps_one_manager_across_logical_stop() {
        let fixture = ServerFixture::new().await;
        let manager = fixture.runtime.server.token_manager.clone();
        assert!(*fixture.runtime.server.is_running.read().await);
        assert_eq!(fixture.get_model_ids().await, vec!["gemini-3.8-flash"]);
        fixture.stop_proxy().await;
        assert!(!*fixture.runtime.server.is_running.read().await);
        assert!(super::probe_health(fixture.config.proxy.port).await);
        let path = fixture.dir.path().join("accounts/fixture-account.json");
        let mut account: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        account["validation_blocked"] = serde_json::json!(true);
        account["validation_blocked_until"] = serde_json::json!(0);
        fs::write(path, serde_json::to_vec(&account).unwrap()).unwrap();
        let before = fixture.snapshot_data_files();
        assert_eq!(fixture.get_model_ids().await, vec!["gemini-3.8-flash"]);
        assert!(Arc::ptr_eq(&manager, &fixture.runtime.server.token_manager));
        assert_eq!(before, fixture.snapshot_data_files());
        fixture.shutdown().await;
    }

    #[test]
    fn server_runtime_startup_overrides_are_explicit_and_prefer_abv_names() {
        let mut config = AppConfig::new();
        let before = serde_json::to_value(&config).unwrap();
        assert!(!super::apply_startup_overrides(&mut config, |_| None).unwrap());
        assert_eq!(before, serde_json::to_value(&config).unwrap());
        let values = std::collections::HashMap::from([
            ("ABV_API_KEY", "primary-key"),
            ("API_KEY", "fallback-key"),
            ("ABV_WEB_PASSWORD", "primary-password"),
            ("WEB_PASSWORD", "fallback-password"),
            ("ABV_AUTH_MODE", "strict"),
            ("AUTH_MODE", "off"),
            ("ABV_BIND_LOCAL_ONLY", "true"),
        ]);
        assert!(super::apply_startup_overrides(&mut config, |key| values
            .get(key)
            .map(|v| v.to_string()))
        .unwrap());
        assert_eq!(config.proxy.api_key, "primary-key");
        assert_eq!(
            config.proxy.admin_password.as_deref(),
            Some("primary-password")
        );
        assert!(matches!(
            config.proxy.auth_mode,
            crate::proxy::ProxyAuthMode::Strict
        ));
        assert!(!config.proxy.allow_lan_access);
        assert!(!super::apply_startup_overrides(&mut config, |key| values
            .get(key)
            .map(|v| v.to_string()))
        .unwrap());
    }

    #[tokio::test]
    async fn server_runtime_starts_without_accounts_and_releases_listener() {
        let fixture = ServerFixture::with_account(false).await;
        assert!(!*fixture.runtime.server.is_running.read().await);
        assert!(fixture.get_model_ids().await.is_empty());
        let port = fixture.config.proxy.port;
        fixture.shutdown().await;
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn server_runtime_sigint_and_sigterm_share_shutdown_and_release_listener() {
        for terminate in [false, true] {
            let fixture = ServerFixture::new().await;
            let port = fixture.config.proxy.port;
            let ServerFixture {
                runtime,
                previous_dir,
                dir,
                ..
            } = fixture;
            let server = runtime.server.clone();
            let scheduler = tokio::spawn(std::future::pending::<()>());
            let watchdog = tokio::spawn(std::future::pending::<()>());
            let scheduler_status = scheduler.abort_handle();
            let watchdog_status = watchdog.abort_handle();
            let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel();
            let (terminate_tx, terminate_rx) = tokio::sync::oneshot::channel();
            if terminate {
                terminate_tx.send(()).unwrap();
            } else {
                interrupt_tx.send(()).unwrap();
            }
            let signal = super::select_shutdown_signal(
                async { interrupt_rx.await.map_err(|e| e.to_string()) },
                async { terminate_rx.await.map_err(|e| e.to_string()) },
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                super::shutdown_after_signal(runtime, scheduler, watchdog, signal),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(server.is_stopped());
            assert!(scheduler_status.is_finished());
            assert!(watchdog_status.is_finished());
            assert!(
                tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                    .await
                    .is_err()
            );
            unsafe {
                match previous_dir {
                    Some(value) => std::env::set_var("ABV_DATA_DIR", value),
                    None => std::env::remove_var("ABV_DATA_DIR"),
                }
            }
            drop(dir);
        }
    }
}
