use serde_json::{Map, Value};

/// 规范 Gemini 请求中的兼容字段，不覆盖原生参数或已声明的消息角色。
pub(crate) fn normalize_request(request: &mut Value) {
    let Some(request) = request.as_object_mut() else {
        return;
    };

    normalize_top_p(request);
    if let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) {
        for content in contents {
            normalize_role(content);
        }
    }
}

/// 顶层 top_p 仅作为别名；原生 generationConfig.topP 优先。
fn normalize_top_p(request: &mut Map<String, Value>) {
    if request
        .get("generationConfig")
        .is_some_and(|config| !config.is_object() && !config.is_null())
    {
        return;
    }

    let Some(top_p) = request.remove("top_p") else {
        return;
    };
    let config = request
        .entry("generationConfig")
        .or_insert_with(|| Value::Object(Map::new()));
    if config.is_null() {
        *config = Value::Object(Map::new());
    }
    if let Some(config) = config.as_object_mut() {
        config.entry("topP").or_insert(top_p);
    }
}

/// 未声明角色的普通内容视为用户输入；工具消息按方向补全，冲突内容交由上游校验。
fn normalize_role(content: &mut Value) {
    let Some(content) = content.as_object_mut() else {
        return;
    };
    let missing_role = match content.get("role") {
        None | Some(Value::Null) => true,
        Some(Value::String(role)) => role.trim().is_empty(),
        _ => false,
    };
    if !missing_role {
        return;
    }
    let Some(parts) = content.get("parts").and_then(Value::as_array) else {
        return;
    };

    let model_content = parts.iter().any(|part| {
        part.get("functionCall").is_some()
            || part.get("function_call").is_some()
            || part.get("executableCode").is_some()
            || part.get("codeExecutionResult").is_some()
            || part.get("toolCall").is_some()
            || part.get("thought").and_then(Value::as_bool) == Some(true)
    });
    let user_content = parts.iter().any(|part| {
        part.get("functionResponse").is_some()
            || part.get("function_response").is_some()
            || part.get("toolResponse").is_some()
    });
    if model_content && user_content {
        return;
    }
    let role = if model_content { "model" } else { "user" };
    content.insert("role".to_string(), Value::String(role.to_string()));
}

#[cfg(test)]
mod tests {
    use super::normalize_request;
    use serde_json::json;

    #[test]
    fn accepts_roleless_probe_with_top_p_alias() {
        let mut request = json!({
            "contents": [{"parts": [{"text": "Hi"}]}],
            "generationConfig": {"maxOutputTokens": 100},
            "top_p": 0.95
        });
        normalize_request(&mut request);
        assert_eq!(
            request,
            json!({
                "contents": [{"role": "user", "parts": [{"text": "Hi"}]}],
                "generationConfig": {"maxOutputTokens": 100, "topP": 0.95}
            })
        );
    }

    #[test]
    fn preserves_native_sampling_and_explicit_conversation_roles() {
        let mut request = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "Question"}]},
                {"role": "model", "parts": [{"text": "Answer"}]},
                {"parts": [{"text": "Follow-up"}]}
            ],
            "systemInstruction": {"parts": [{"text": "System"}]},
            "generationConfig": {"topP": 0.7, "temperature": 0.4},
            "top_p": 0.95
        });
        let mut expected = request.clone();
        expected.as_object_mut().unwrap().remove("top_p");
        expected["contents"][2]["role"] = json!("user");
        normalize_request(&mut request);
        assert_eq!(request, expected);
    }

    #[test]
    fn preserves_tool_roundtrip_direction_and_payloads() {
        let mut request = json!({
            "contents": [
                {"role": "user", "parts": [{"text": "Check weather"}]},
                {"parts": [{"functionCall": {"name": "weather", "args": {"city": "X"}}, "thoughtSignature": "signature"}]},
                {"parts": [{"functionResponse": {"name": "weather", "response": {"temperature": 20}}}]},
                {"role": "model", "parts": [{"text": "20 degrees"}]}
            ]
        });
        let mut expected = request.clone();
        expected["contents"][1]["role"] = json!("model");
        expected["contents"][2]["role"] = json!("user");
        normalize_request(&mut request);
        assert_eq!(request, expected);
    }

    #[test]
    fn recognizes_snake_case_tools_and_model_thoughts() {
        let mut request = json!({"contents": [
            {"role": "", "parts": [{"function_call": {"name": "tool"}}]},
            {"role": null, "parts": [{"function_response": {"name": "tool"}}]},
            {"parts": [{"thought": true, "text": "Existing reasoning"}]}
        ]});
        normalize_request(&mut request);
        assert_eq!(request["contents"][0]["role"], "model");
        assert_eq!(request["contents"][1]["role"], "user");
        assert_eq!(request["contents"][2]["role"], "model");
    }

    #[test]
    fn handles_missing_or_null_generation_config_without_changing_zero() {
        for config in [None, Some(serde_json::Value::Null)] {
            let mut request = json!({"top_p": 0});
            if let Some(config) = config {
                request["generationConfig"] = config;
            }
            normalize_request(&mut request);
            assert_eq!(request, json!({"generationConfig": {"topP": 0}}));
        }
    }

    #[test]
    fn leaves_malformed_or_ambiguous_input_for_validation() {
        for mut request in [
            json!(null),
            json!({"top_p": 0.5, "generationConfig": "invalid"}),
            json!({"contents": [{"role": 3, "parts": [{"text": "Hi"}]}]}),
            json!({"contents": [{"parts": [
                {"functionCall": {"name": "tool"}},
                {"functionResponse": {"name": "tool"}}
            ]}]}),
        ] {
            let before = request.clone();
            normalize_request(&mut request);
            assert_eq!(request, before);
        }
    }

    #[test]
    fn normalization_is_idempotent() {
        let mut request = json!({
            "top_p": 0.9,
            "contents": [{"parts": [{"inlineData": {"mimeType": "image/png", "data": "image"}}]}]
        });
        normalize_request(&mut request);
        let once = request.clone();
        normalize_request(&mut request);
        assert_eq!(request, once);
    }
}
