use serde_json::{json, Value};

const MODEL: &str = "claude-opus-5-5";

fn with_control_sources(mut test: impl FnMut()) {
    use crate::proxy::config::*;
    let _lock = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    struct Restore(ThinkingBudgetConfig);
    impl Drop for Restore {
        fn drop(&mut self) {
            update_thinking_budget_config(self.0.clone());
        }
    }
    let _restore = Restore(get_thinking_budget_config());
    for control_source in [
        ThinkingControlSource::Gateway,
        ThinkingControlSource::Client,
    ] {
        update_thinking_budget_config(ThinkingBudgetConfig {
            control_source,
            ..Default::default()
        });
        test();
    }
}

#[test]
fn opus_5_5_protocol_forced_tools_across_adapters() {
    use crate::proxy::mappers::common_utils::{
        map_claude_tool_choice_to_gemini, map_openai_tool_choice_to_gemini,
    };
    let configs = [
        map_claude_tool_choice_to_gemini(&json!({"type":"any"})).unwrap(),
        map_claude_tool_choice_to_gemini(&json!({"type":"tool","name":"lookup"})).unwrap(),
        map_openai_tool_choice_to_gemini(&json!("required")).unwrap(),
        map_openai_tool_choice_to_gemini(&json!({"type":"function","function":{"name":"lookup"}}))
            .unwrap(),
        map_openai_tool_choice_to_gemini(&json!({"type":"function","name":"lookup"})).unwrap(),
        json!({"functionCallingConfig":{"mode":"ANY"}}),
        json!({"function_calling_config":{"mode":"ANY"}}),
    ];
    for config in configs {
        let body = json!({"request":{"toolConfig":config}});
        for model in [
            MODEL,
            "models/anthropic/claude-opus-5.5",
            "models/models/anthropic/anthropic/claude-opus-5.5",
        ] {
            let error = super::InboundThinkingPipeline::validate_request_constraints(model, &body)
                .unwrap_err();
            assert!(error.contains("forced tool choice"));
        }
        assert!(
            super::InboundThinkingPipeline::validate_request_constraints("future-model-9", &body)
                .is_ok()
        );
    }
    for mode in ["AUTO", "NONE"] {
        let body = json!({"tool_config":{"function_calling_config":{"mode":mode}}});
        assert!(super::InboundThinkingPipeline::validate_request_constraints(MODEL, &body).is_ok());
    }
}

fn assert_adaptive(body: &Value) {
    assert_eq!(body["model"], MODEL);
    assert_eq!(
        body["request"]["generationConfig"]["thinkingConfig"],
        json!({"includeThoughts": true})
    );
}

#[test]
fn opus_5_5_protocol_claude_disabled_and_manual_budget() {
    with_control_sources(|| {
        for thinking in [
            json!({"type":"disabled"}),
            json!({"type":"enabled","budget_tokens":4096}),
        ] {
            let request = serde_json::from_value(json!({
                "model": MODEL, "max_tokens": 100, "messages": [{"role":"user","content":"Hello"}],
                "thinking": thinking
            }))
            .unwrap();
            let body = crate::proxy::mappers::claude::transform_claude_request_in(
                &request,
                "project",
                false,
                None,
                "opus-test",
                None,
            )
            .unwrap();
            assert_adaptive(&body);
        }
    });
}

#[test]
fn opus_5_5_protocol_openai_chat_and_responses() {
    with_control_sources(|| {
        for responses in [false, true] {
            for reasoning in [json!({"effort":"none"}), json!({"max_tokens":4096})] {
                let request = serde_json::from_value(json!({
                    "model": MODEL, "messages": [{"role":"user","content":"Hello"}],
                    "reasoning": reasoning, "thinking": {"type":"disabled"}
                }))
                .unwrap();
                let (body, ..) =
                    crate::proxy::mappers::openai::transform_openai_request_with_session(
                        &request,
                        "project",
                        MODEL,
                        None,
                        "opus-test",
                        None,
                        responses,
                    );
                assert_adaptive(&body);
            }
        }
    });
}

#[test]
fn opus_5_5_protocol_gemini_disabled_and_manual_budget() {
    with_control_sources(|| {
        for thinking in [
            json!({"includeThoughts":false}),
            json!({"thinkingBudget":4096}),
        ] {
            let request = json!({
                "contents": [{"role":"user","parts":[{"text":"Hello"}]}],
                "generationConfig": {"thinkingConfig": thinking}
            });
            let body = crate::proxy::mappers::gemini::wrap_request_v2(
                &request, "project", MODEL, None, None, None, None, None, None,
            );
            assert_adaptive(&body);
        }
    });
}

#[test]
fn opus_5_5_protocol_unknown_model_passthrough() {
    let model = "future-model-9";
    assert_eq!(
        crate::proxy::common::model_mapping::map_claude_model_to_gemini(model),
        model
    );
    assert!(!crate::proxy::model_specs::is_adaptive_thinking_model(
        model
    ));
}

#[test]
fn opus_5_5_protocol_pipeline_removes_equivalent_budget_fields() {
    let mut request = json!({
        "thinkingConfig": {"thinkingBudget": 1024},
        "thinking_config": {"thinking_budget": 512},
        "generationConfig": {
            "thinking_config": {"budget_tokens": 4096, "include_thoughts":false},
            "thinkingConfig": {"maxTokens":8192, "includeThoughts":false}
        }
    });
    super::InboundThinkingPipeline::align_google_request_prefix_topology_with_model(
        &mut request,
        MODEL,
        None,
    );
    assert_eq!(
        request["generationConfig"]["thinkingConfig"],
        json!({"includeThoughts":true})
    );
    assert!(request["generationConfig"].get("thinking_config").is_none());
    assert!(request.get("thinking_config").is_none());
}

#[test]
fn opus_5_5_protocol_tool_choices_survive_full_mapping() {
    with_control_sources(|| {
        for (claude_choice, openai_choice, gemini_mode, forced) in [
            (json!({"type":"auto"}), json!("auto"), "AUTO", false),
            (json!({"type":"none"}), json!("none"), "NONE", false),
            (json!({"type":"any"}), json!("required"), "ANY", true),
            (
                json!({"type":"tool","name":"lookup"}),
                json!({"type":"function","name":"lookup"}),
                "ANY",
                true,
            ),
        ] {
            let claude = serde_json::from_value(json!({
                "model": MODEL, "max_tokens": 100, "messages": [{"role":"user","content":"Hello"}],
                "tools": [{"name":"lookup","input_schema":{"type":"object"}}], "tool_choice":claude_choice
            })).unwrap();
            let body = crate::proxy::mappers::claude::transform_claude_request_in(
                &claude,
                "project",
                false,
                None,
                "opus-tools",
                None,
            )
            .unwrap();
            assert_adaptive(&body);
            assert_eq!(
                super::InboundThinkingPipeline::validate_request_constraints(MODEL, &body).is_err(),
                forced
            );
            for responses in [false, true] {
                let openai = serde_json::from_value(json!({
                    "model": MODEL, "messages": [{"role":"user","content":"Hello"}],
                    "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}], "tool_choice":openai_choice
                })).unwrap();
                let (body, ..) =
                    crate::proxy::mappers::openai::transform_openai_request_with_session(
                        &openai,
                        "project",
                        MODEL,
                        None,
                        "opus-tools",
                        None,
                        responses,
                    );
                assert_adaptive(&body);
                assert_eq!(
                    super::InboundThinkingPipeline::validate_request_constraints(MODEL, &body)
                        .is_err(),
                    forced
                );
            }
            let gemini = json!({
                "contents":[{"role":"user","parts":[{"text":"Hello"}]}],
                "tools":[{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}],
                "tool_config":{"function_calling_config":{"mode":gemini_mode}}
            });
            let body = crate::proxy::mappers::gemini::wrap_request_v2(
                &gemini, "project", MODEL, None, None, None, None, None, None,
            );
            assert_adaptive(&body);
            assert_eq!(
                super::InboundThinkingPipeline::validate_request_constraints(MODEL, &body).is_err(),
                forced
            );
        }
    });
}
