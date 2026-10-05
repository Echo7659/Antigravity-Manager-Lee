use crate::{models::AppConfig, modules, proxy};
use std::{sync::Arc, time::Duration};

struct WatchdogHandle {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatchdogHandle {
    fn disabled() -> Self {
        Self {
            stop: None,
            thread: None,
        }
    }

    fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("Health watchdog thread panicked during shutdown");
        }
    }
}

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

/// 只读持久配置中的当前凭据，缺省时回退环境变量，不执行配置迁移。
fn health_check_api_key(env: impl Fn(&str) -> Option<String>) -> Result<Option<String>, String> {
    let fallback = || {
        env("ABV_API_KEY")
            .filter(|key| !key.trim().is_empty())
            .or_else(|| env("API_KEY").filter(|key| !key.trim().is_empty()))
    };
    let path = modules::account::resolve_data_dir_read_only()?.join("gui_config.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(fallback()),
        Err(error) => {
            return Err(format!(
                "Failed to read health-check configuration: {error}"
            ))
        }
    };
    let config: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid health-check configuration JSON".to_string())?;
    Ok(config
        .get("proxy")
        .and_then(|proxy| proxy.get("api_key"))
        .and_then(serde_json::Value::as_str)
        .filter(|key| !key.trim().is_empty())
        .map(str::to_string)
        .or_else(fallback))
}

async fn probe_server_health(server: &proxy::AxumServer, port: u16) -> bool {
    let api_key = server.current_api_key().await;
    probe_health(port, Some(&api_key)).await
}

async fn probe_health(port: u16, api_key: Option<&str>) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let probe = async {
        let mut stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let mut request =
            String::from("GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        if let Some(key) = api_key.filter(|key| !key.is_empty()) {
            if key.contains(['\r', '\n']) {
                return Ok(false);
            }
            request.push_str("Authorization: Bearer ");
            request.push_str(key);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await?;
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

fn run_blocking_health_check(port: u16, api_key: Option<&str>, timeout: Duration) -> bool {
    use std::io::{Read, Write};

    let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        return false;
    }

    let mut request =
        String::from("GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        if key.contains(['\r', '\n']) {
            return false;
        }
        request.push_str("Authorization: Bearer ");
        request.push_str(key);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = [0_u8; 256];
    let Ok(read) = stream.read(&mut response) else {
        return false;
    };
    response[..read].starts_with(b"HTTP/1.1 200 ") || response[..read].starts_with(b"HTTP/1.0 200 ")
}

fn record_watchdog_probe(failures: &mut u8, healthy: bool) -> bool {
    if healthy {
        *failures = 0;
        return false;
    }
    *failures = failures.saturating_add(1);
    *failures >= 3
}

fn spawn_health_watchdog_loop<Probe, Exit>(
    startup_delay: Duration,
    interval: Duration,
    mut probe: Probe,
    mut exit: Exit,
) -> Result<WatchdogHandle, String>
where
    Probe: FnMut() -> bool + Send + 'static,
    Exit: FnMut() + Send + 'static,
{
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("antigravity-health-watchdog".to_string())
        .spawn(move || {
            if !matches!(
                stop_rx.recv_timeout(startup_delay),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                return;
            }

            let mut failures = 0;
            loop {
                if record_watchdog_probe(&mut failures, probe()) {
                    exit();
                    return;
                }
                if !matches!(
                    stop_rx.recv_timeout(interval),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    return;
                }
            }
        })
        .map_err(|error| format!("Failed to start health watchdog: {error}"))?;
    Ok(WatchdogHandle {
        stop: Some(stop_tx),
        thread: Some(thread),
    })
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
    watchdog: WatchdogHandle,
    signal: impl std::future::Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let result = signal.await;
    scheduler.abort();
    let _ = scheduler.await;
    watchdog.stop();
    runtime.shutdown().await;
    result
}

pub async fn run() -> Result<(), String> {
    if std::env::args().any(|arg| arg == "--health-check") {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8045);
        let api_key = health_check_api_key(|key| std::env::var(key).ok())?;
        return if probe_health(port, api_key.as_deref()).await {
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
    let watchdog = if watchdog_enabled {
        spawn_health_watchdog_loop(
            Duration::from_secs(30),
            Duration::from_secs(15),
            move || {
                let api_key = match health_check_api_key(|key| std::env::var(key).ok()) {
                    Ok(api_key) => api_key,
                    Err(error) => {
                        tracing::warn!("Health watchdog could not load API key: {error}");
                        return false;
                    }
                };
                run_blocking_health_check(port, api_key.as_deref(), Duration::from_secs(5))
            },
            || {
                tracing::error!("HTTP runtime is unresponsive; exiting for supervisor restart");
                std::process::exit(1);
            },
        )?
    } else {
        WatchdogHandle::disabled()
    };
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
        assert!(super::probe_health(fixture.config.proxy.port, None).await);
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
    async fn server_runtime_strict_health_probe_requires_valid_key() {
        let fixture = ServerFixture::new().await;
        let mut config = fixture.config.clone();
        config.proxy.auth_mode = crate::proxy::ProxyAuthMode::Strict;
        fixture.save_config(&config).await;
        assert!(super::probe_health(config.proxy.port, Some("fixture-key")).await);
        assert!(!super::probe_health(config.proxy.port, None).await);
        assert!(!super::probe_health(config.proxy.port, Some("wrong-key")).await);
        config.proxy.api_key = "rotated-fixture-key".into();
        fixture.save_config(&config).await;
        assert!(super::probe_server_health(&fixture.runtime.server, config.proxy.port).await);
        assert!(!super::probe_health(config.proxy.port, Some("fixture-key")).await);
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn server_runtime_cli_health_key_reads_config_without_modifying_files() {
        let fixture = ServerFixture::new().await;
        let mut config = fixture.config.clone();
        config.proxy.auth_mode = crate::proxy::ProxyAuthMode::Strict;
        config.proxy.api_key = "rotated-fixture-key".into();
        fixture.save_config(&config).await;
        let path = fixture.dir.path().join("gui_config.json");
        let before = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let key = super::health_check_api_key(|name| {
            (name == "ABV_API_KEY").then(|| "fixture-key".into())
        })
        .unwrap();
        assert_eq!(key.as_deref(), Some("rotated-fixture-key"));
        assert!(super::probe_health(config.proxy.port, key.as_deref()).await);
        assert!(!super::probe_health(config.proxy.port, Some("fixture-key")).await);
        assert!(!super::probe_health(config.proxy.port, Some("bad\r\nkey")).await);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        fs::remove_file(&path).unwrap();
        assert_eq!(
            super::health_check_api_key(|name| match name {
                "ABV_API_KEY" => Some("primary".into()),
                "API_KEY" => Some("fallback".into()),
                _ => None,
            })
            .unwrap()
            .as_deref(),
            Some("primary")
        );
        assert_eq!(
            super::health_check_api_key(|name| (name == "API_KEY").then(|| "fallback".into()))
                .unwrap()
                .as_deref(),
            Some("fallback")
        );
        assert!(!path.exists());
        fs::write(&path, br#"{"proxy":{}}"#).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            super::health_check_api_key(|name| (name == "API_KEY").then(|| "fallback".into()))
                .unwrap()
                .as_deref(),
            Some("fallback")
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        fixture.shutdown().await;
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
            let watchdog = super::spawn_health_watchdog_loop(
                std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(60),
                || true,
                || {},
            )
            .unwrap();
            let scheduler_status = scheduler.abort_handle();
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

    #[test]
    fn watchdog_failure_counter_resets_and_trips_on_third_failure() {
        let mut failures = 0;
        assert!(!super::record_watchdog_probe(&mut failures, false));
        assert!(!super::record_watchdog_probe(&mut failures, false));
        assert!(super::record_watchdog_probe(&mut failures, false));
        assert!(!super::record_watchdog_probe(&mut failures, true));
        assert_eq!(failures, 0);
    }

    #[tokio::test]
    async fn blocking_health_probe_matches_async_health_semantics() {
        let fixture = ServerFixture::new().await;
        let port = fixture.config.proxy.port;
        assert!(tokio::task::spawn_blocking(move || {
            super::run_blocking_health_check(port, None, std::time::Duration::from_secs(2))
        })
        .await
        .unwrap());
        fixture.shutdown().await;
    }

    #[test]
    fn watchdog_thread_is_independent_and_interruptible() {
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let exits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe_count = probes.clone();
        let exit_count = exits.clone();
        let watchdog = super::spawn_health_watchdog_loop(
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(1),
            move || {
                probe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                false
            },
            move || {
                exit_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while exits.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 3);
        watchdog.stop();
    }
}
