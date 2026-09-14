use serde_json::Value;

fn counter(usage: &Value, names: &[&str]) -> Option<u32> {
    names.iter().find_map(|name| {
        usage
            .get(*name)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
    })
}

/// 返回 Gemini 的总生成 token，包含思考；已包含思考的旧上游计数不重复相加。
pub(crate) fn gemini_output_tokens(usage: &Value) -> Option<u32> {
    let output = counter(usage, &["total_output_tokens", "candidatesTokenCount"]);
    let thoughts = counter(
        usage,
        &[
            "total_thought_tokens",
            "totalThoughtTokens",
            "thoughtsTokenCount",
        ],
    );
    if output.is_none() && thoughts.is_none() {
        return None;
    }
    let output = output.unwrap_or(0);
    let thoughts = thoughts.unwrap_or(0);
    let generated = output.saturating_add(thoughts);

    if usage.get("total_output_tokens").is_some() {
        return Some(
            generated.saturating_add(counter(usage, &["total_tool_use_tokens"]).unwrap_or(0)),
        );
    }

    // 上游总量能够区分独立思考计数与已合并思考计数的两种旧格式。
    if let (Some(total), Some(prompt)) = (
        counter(usage, &["totalTokenCount"]),
        counter(usage, &["promptTokenCount"]),
    ) {
        if let Some(reported_output) = total.checked_sub(prompt) {
            if reported_output >= output && reported_output >= thoughts {
                return Some(reported_output);
            }
        }
    }
    Some(generated)
}

/// 合并流式累计计数，后续缺失字段或占位零值不覆盖已收到的有效值。
pub(crate) fn merge_counter(current: Option<u32>, incoming: Option<u32>) -> Option<u32> {
    current.into_iter().chain(incoming).max()
}

/// 合并同一请求的累计用量快照，保留后续帧未重复携带的字段。
pub(crate) fn merge_usage_metadata(current: &mut Value, incoming: &Value) {
    if incoming.is_null() {
        return;
    }
    if let (Some(previous), Some(next)) = (current.as_u64(), incoming.as_u64()) {
        *current = Value::from(previous.max(next));
        return;
    }
    if let (Some(current), Some(incoming)) = (current.as_object_mut(), incoming.as_object()) {
        for (key, value) in incoming {
            merge_usage_metadata(current.entry(key.clone()).or_insert(Value::Null), value);
        }
    } else {
        *current = incoming.clone();
    }
}

/// 提取流中的结构化失败事件；HTTP 响应头已发送时仍保留实际失败信息。
pub(crate) fn stream_error(event: &Value) -> Option<(u16, String)> {
    let error = event
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            event
                .get("response")?
                .get("error")
                .filter(|error| !error.is_null())
        });
    let Some(error) = error else {
        let failed_choice = event
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                choices
                    .iter()
                    .any(|choice| choice["finish_reason"] == "error")
            });
        return failed_choice.then(|| {
            (
                502,
                "Upstream stream terminated with finish_reason=error".to_string(),
            )
        });
    };
    let status = error
        .get("code")
        .and_then(Value::as_u64)
        .filter(|code| (400..=599).contains(code))
        .map(|code| code as u16)
        .unwrap_or(502);
    Some((status, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compat_thinking_only_is_not_zero_generated_tokens() {
        assert_eq!(
            gemini_output_tokens(&json!({
                "promptTokenCount": 9868, "candidatesTokenCount": 0,
                "thoughtsTokenCount": 722, "totalTokenCount": 10590
            })),
            Some(722)
        );
    }

    #[test]
    fn compat_handles_separate_and_already_merged_legacy_thoughts() {
        assert_eq!(
            gemini_output_tokens(&json!({
                "promptTokenCount": 100, "candidatesTokenCount": 20,
                "thoughtsTokenCount": 30, "totalTokenCount": 150
            })),
            Some(50)
        );
        assert_eq!(
            gemini_output_tokens(&json!({
                "promptTokenCount": 100, "candidatesTokenCount": 50,
                "thoughtsTokenCount": 30, "totalTokenCount": 150
            })),
            Some(50)
        );
    }

    #[test]
    fn compat_preserves_interactions_and_unknown_usage() {
        assert_eq!(
            gemini_output_tokens(&json!({
                "total_input_tokens": 7, "total_output_tokens": 20,
                "total_thought_tokens": 22, "total_tool_use_tokens": 3
            })),
            Some(45)
        );
        assert_eq!(
            gemini_output_tokens(&json!({"promptTokenCount": 100})),
            None
        );
        assert_eq!(
            gemini_output_tokens(&json!({"thoughtsTokenCount": 12})),
            Some(12)
        );
        assert_eq!(
            gemini_output_tokens(&json!({"candidatesTokenCount": 0})),
            Some(0)
        );
    }

    #[test]
    fn compat_invalid_totals_do_not_erase_reported_thoughts() {
        assert_eq!(
            gemini_output_tokens(&json!({
                "promptTokenCount": 100, "candidatesTokenCount": 0,
                "thoughtsTokenCount": 12, "totalTokenCount": 100
            })),
            Some(12)
        );
    }

    #[test]
    fn compat_partial_stream_updates_preserve_cumulative_usage() {
        let mut value = None;
        for incoming in [Some(0), Some(12), None, Some(0), Some(30)] {
            value = merge_counter(value, incoming);
        }
        assert_eq!(value, Some(30));
        assert_eq!(merge_counter(None, None), None);
    }

    #[test]
    fn compat_merges_split_usage_and_ignores_terminal_placeholders() {
        let mut usage = json!({});
        for update in [
            json!({"promptTokenCount": 100, "candidatesTokenCount": 0}),
            json!({"thoughtsTokenCount": 12}),
            json!({"candidatesTokenCount": 20, "totalTokenCount": 132}),
            json!({"promptTokenCount": null, "candidatesTokenCount": 0}),
        ] {
            merge_usage_metadata(&mut usage, &update);
        }
        assert_eq!(usage["promptTokenCount"], 100);
        assert_eq!(gemini_output_tokens(&usage), Some(32));
        let mut details = json!({"output_tokens_details": {"reasoning_tokens": 10}});
        merge_usage_metadata(
            &mut details,
            &json!({"output_tokens_details": {"reasoning_tokens": 0}}),
        );
        assert_eq!(details["output_tokens_details"]["reasoning_tokens"], 10);
    }

    #[test]
    fn compat_stream_failures_are_distinct_from_successful_refusals() {
        assert_eq!(
            stream_error(&json!({"error": {"code": 429, "message": "limited"}}))
                .unwrap()
                .0,
            429
        );
        assert_eq!(
            stream_error(&json!({"response": {"error": {"code": "empty_response"}}}))
                .unwrap()
                .0,
            502
        );
        assert!(stream_error(&json!({"candidates": [{"finishReason": "SAFETY"}]})).is_none());
        assert!(stream_error(&json!({"response": {"error": null}})).is_none());
    }
}
