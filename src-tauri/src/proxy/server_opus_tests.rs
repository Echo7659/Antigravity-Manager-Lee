use super::*;
use crate::proxy::{
    config::ProxyConfig,
    handlers,
    monitor::{ProxyMonitor, ProxyStats},
};
use std::{collections::VecDeque, sync::atomic::AtomicBool};

fn empty_state() -> AppState {
    let config = ProxyConfig::default();
    let pool_config = Arc::new(RwLock::new(config.proxy_pool.clone()));
    AppState {
        config_update: Arc::new(tokio::sync::Mutex::new(())),
        token_manager: Arc::new(TokenManager::new(account::get_data_dir().unwrap())),
        custom_mapping: Arc::new(RwLock::new(HashMap::new())),
        request_timeout: 5,
        thought_signature_map: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        upstream_proxy: Arc::new(RwLock::new(config.upstream_proxy.clone())),
        upstream: Arc::new(crate::proxy::upstream::client::UpstreamClient::new(
            Some(config.upstream_proxy.clone()),
            None,
        )),
        monitor: Arc::new(ProxyMonitor {
            logs: RwLock::new(VecDeque::new()),
            stats: RwLock::new(ProxyStats::default()),
            max_logs: 10,
            enabled: Arc::new(AtomicBool::new(false)),
            capture_health_logs: Arc::new(AtomicBool::new(false)),
        }),
        experimental: Arc::new(RwLock::new(config.experimental.clone())),
        debug_logging: Arc::new(RwLock::new(config.debug_logging.clone())),
        switching: Arc::new(RwLock::new(false)),
        account_service: Arc::new(crate::modules::account_service::AccountService::new()),
        security: Arc::new(RwLock::new(
            crate::proxy::ProxySecurityConfig::from_proxy_config(&config),
        )),
        is_running: Arc::new(RwLock::new(true)),
        port: 0,
        proxy_pool_state: pool_config.clone(),
        proxy_pool_manager: Arc::new(crate::proxy::proxy_pool::ProxyPoolManager::new(pool_config)),
        only_raw_quota_models: Arc::new(RwLock::new(true)),
        image_scheduler: ImageScheduler::new(Vec::new(), 1),
    }
}

async fn assert_forced_error(response: Response, protocol: &str, model: &str) {
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "{protocol}: {model}"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("forced tool choice"),
        "{protocol}: {model}: {bytes:?}"
    );
}

fn assert_mapped_model(response: Response, expected: &str) {
    assert_eq!(
        response
            .headers()
            .get("X-Mapped-Model")
            .and_then(|v| v.to_str().ok()),
        Some(expected),
        "status: {}",
        response.status()
    );
}

#[test]
fn opus_5_5_claude_handler_routes_configured_alias_from_original_effort() {
    // 全局思考预算配置锁覆盖完整异步调用，避免并行测试改写控制模式。
    let _config_lock = crate::proxy::config::TEST_CONFIG_LOCK.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(opus_5_5_claude_handler_routes_configured_alias_body());
}

async fn opus_5_5_claude_handler_routes_configured_alias_body() {
    let _dir = crate::proxy::monitor::prompt_log_tests::TestDataDir::new();
    let saved_config = crate::proxy::config::get_thinking_budget_config();
    let state = empty_state();
    {
        let mut mapping = state.custom_mapping.write().await;
        mapping.insert("client-opus".into(), "claude-opus-5-5".into());
        mapping.insert("claude-opus-5-5".into(), "claude-opus-5-5-low".into());
    }

    for control_source in [
        crate::proxy::config::ThinkingControlSource::Gateway,
        crate::proxy::config::ThinkingControlSource::Client,
    ] {
        let mut config = saved_config.clone();
        config.control_source = control_source;
        crate::proxy::config::update_thinking_budget_config(config);
        for (effort, expected) in [
            (Some("low"), "claude-opus-5-5-low"),
            (Some("medium"), "claude-opus-5-5-medium"),
            (Some("high"), "claude-opus-5-5-high"),
            (Some("unrecognized"), "claude-opus-5-5-high"),
            (None, "claude-opus-5-5-high"),
        ] {
            let mut body = json!({"model":"client-opus","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]});
            if let Some(effort) = effort {
                body["reasoning_effort"] = json!(effort);
            }
            let response = handlers::claude::handle_messages(
                State(state.clone()),
                HeaderMap::new(),
                None,
                None,
                Json(body),
            )
            .await;
            assert_mapped_model(response, expected);
        }
    }
    crate::proxy::config::update_thinking_budget_config(saved_config);
}

#[tokio::test]
async fn opus_5_5_handlers_preserve_mapping_boundaries_and_physical_ids() {
    let _dir = crate::proxy::monitor::prompt_log_tests::TestDataDir::new();
    let state = empty_state();
    let retired = "opus-5-5-retired-handler-test";
    crate::proxy::common::model_mapping::DYNAMIC_MODEL_FORWARDING_RULES
        .insert(retired.into(), "claude-opus-5-5".into());
    {
        let mut mapping = state.custom_mapping.write().await;
        mapping.insert("client-opus".into(), "claude-opus-5-5".into());
        mapping.insert("claude-opus-5-5".into(), "claude-opus-5-5-low".into());
    }

    for (model, effort, expected) in [
        ("client-opus", Some("medium"), "claude-opus-5-5-medium"),
        ("client-opus", None, "claude-opus-5-5-high"),
        (retired, Some("medium"), "claude-opus-5-5-medium"),
        ("claude-opus-5-5-high", Some("low"), "claude-opus-5-5-high"),
    ] {
        let mut chat = json!({"model":model,"messages":[{"role":"user","content":"Hello"}]});
        if let Some(effort) = effort {
            chat["reasoning_effort"] = json!(effort);
        }
        let response = handlers::openai::handle_chat_completions(
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(chat),
        )
        .await
        .into_response();
        assert_mapped_model(response, expected);
    }

    for (field, expected) in [
        ("reasoningEffort", "claude-opus-5-5-medium"),
        ("thinkingLevel", "claude-opus-5-5-medium"),
        ("thinking_level", "claude-opus-5-5-medium"),
    ] {
        let mut body = json!({"model":"client-opus","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]});
        body[field] = json!("medium");
        let response = handlers::claude::handle_messages(
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(body),
        )
        .await;
        assert_mapped_model(response, expected);
    }
    for (model, effort, expected) in [
        ("client-opus", "medium", "claude-opus-5-5-medium"),
        (retired, "medium", "claude-opus-5-5-medium"),
        ("claude-opus-5-5-high", "low", "claude-opus-5-5-high"),
    ] {
        let body = json!({"model":model,"max_tokens":64,"messages":[{"role":"user","content":"Hello"}],"output_config":{"effort":effort}});
        let response = handlers::claude::handle_messages(
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(body),
        )
        .await;
        assert_mapped_model(response, expected);
    }
    crate::proxy::common::model_mapping::DYNAMIC_MODEL_FORWARDING_RULES.remove(retired);
}

#[tokio::test]
async fn opus_5_5_protocol_handlers_reject_alias_forced_tools_before_account_selection() {
    let _dir = crate::proxy::monitor::prompt_log_tests::TestDataDir::new();
    let state = empty_state();
    let retired = "opus-handler-retired-test";
    crate::proxy::common::model_mapping::DYNAMIC_MODEL_FORWARDING_RULES
        .insert(retired.into(), "models/anthropic/claude-opus-5.5".into());
    state.custom_mapping.write().await.insert(
        "client-opus".into(),
        "models/anthropic/claude-opus-5.5".into(),
    );
    for model in [
        "claude-opus-5-5",
        "anthropic/claude-opus-5-5",
        "claude-opus-5.5",
        "models/anthropic/claude-opus-5.5",
        "models/models/anthropic/anthropic/claude-opus-5.5",
        "client-opus",
        retired,
    ] {
        let chat = json!({"model":model,"messages":[{"role":"user","content":"Hello"}],"tool_choice":"required"});
        let chat = handlers::openai::handle_chat_completions(
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(chat),
        )
        .await
        .into_response();
        assert_forced_error(chat, "chat", model).await;
        let responses = json!({"model":model,"input":[{"role":"user","content":"Hello"}],"tool_choice":{"type":"function","name":"lookup"},"store":false});
        let responses = handlers::openai::handle_completions(
            axum::extract::OriginalUri("/v1/responses".parse().unwrap()),
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(responses),
        )
        .await;
        assert_forced_error(responses, "responses", model).await;
        let claude = json!({"model":model,"max_tokens":64,"messages":[{"role":"user","content":"Hello"}],"tool_choice":{"type":"any"}});
        let claude = handlers::claude::handle_messages(
            State(state.clone()),
            HeaderMap::new(),
            None,
            None,
            Json(claude),
        )
        .await;
        assert_forced_error(claude, "claude", model).await;
        let gemini = json!({"contents":[{"role":"user","parts":[{"text":"Hello"}]}],"toolConfig":{"functionCallingConfig":{"mode":"ANY"}}});
        let gemini = handlers::gemini::handle_generate(
            State(state.clone()),
            Path(format!("{model}:generateContent")),
            HeaderMap::new(),
            None,
            None,
            Json(gemini),
        )
        .await
        .into_response();
        assert_forced_error(gemini, "gemini", model).await;
    }
    crate::proxy::common::model_mapping::DYNAMIC_MODEL_FORWARDING_RULES.remove(retired);
}
