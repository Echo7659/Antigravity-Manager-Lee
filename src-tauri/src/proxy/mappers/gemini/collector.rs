// Gemini Stream Collector
// Used for auto-converting streaming responses to JSON for non-streaming requests

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use serde_json::{json, Value};
use tracing::debug;

use crate::proxy::SignatureCache; // Assuming this is available at crate root or re-exported

/// Collects a Gemini SSE stream into a complete Gemini Response Value
/// ALSO performs signature caching side-effect
pub async fn collect_stream_to_json<S, E>(mut stream: S, session_id: &str) -> Result<Value, String>
where
    S: futures::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut collected_response = json!({
        "candidates": [
            {
                "content": {
                    "parts": [],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }
        ]
    });

    let mut content_parts: Vec<Value> = Vec::new(); // To accumulate parts
    let mut usage_metadata: Option<Value> = None;
    let mut finish_reason: Option<String> = None;
    let mut prompt_feedback: Option<Value> = None;
    let mut buffer = BytesMut::new();

    loop {
        match stream.next().await {
            Some(chunk) => {
                buffer.extend_from_slice(&chunk.map_err(|e| format!("Stream error: {}", e))?)
            }
            None if !buffer.is_empty() => buffer.extend_from_slice(b"\n"),
            None => break,
        }
        while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
            let line_bytes = buffer.split_to(position + 1);
            let line = std::str::from_utf8(&line_bytes)
                .map_err(|e| format!("Invalid stream UTF-8: {}", e))?
                .trim();
            if line.starts_with("data: ") {
                let json_part = line.trim_start_matches("data: ").trim();
                if json_part == "[DONE]" {
                    continue;
                }

                if let Ok(mut json) = serde_json::from_str::<Value>(json_part) {
                    // Unwrap v1internal response wrapper similar to handler
                    let actual_data =
                        if let Some(inner) = json.get_mut("response").map(|v| v.take()) {
                            inner
                        } else {
                            json
                        };

                    if let Some(error) = actual_data.get("error").filter(|error| !error.is_null()) {
                        return Err(format!("Upstream stream error: {}", error));
                    }
                    if let Some(feedback) = actual_data.get("promptFeedback") {
                        prompt_feedback = Some(feedback.clone());
                    }

                    // 1. Capture Usage
                    if let Some(usage) = actual_data.get("usageMetadata") {
                        let current = usage_metadata.get_or_insert_with(|| json!({}));
                        crate::proxy::mappers::usage::merge_usage_metadata(current, usage);
                    }

                    // 2. Capture Content & Signature
                    if let Some(candidates) =
                        actual_data.get("candidates").and_then(|c| c.as_array())
                    {
                        if let Some(candidate) = candidates.first() {
                            // Update finish reason if present
                            if let Some(fr) = candidate.get("finishReason").and_then(|v| v.as_str())
                            {
                                finish_reason = Some(fr.to_string());
                            }

                            if let Some(parts) = candidate
                                .get("content")
                                .and_then(|c| c.get("parts"))
                                .and_then(|p| p.as_array())
                            {
                                for part in parts {
                                    // Signature Caching
                                    if let Some(sig) =
                                        part.get("thoughtSignature").and_then(|s| s.as_str())
                                    {
                                        // Cache it!
                                        SignatureCache::global().cache_session_signature(
                                            session_id,
                                            sig.to_string(),
                                            1,
                                        );
                                        debug!("[Gemini-AutoConverter] Cached signature (len: {}) for session: {}", sig.len(), session_id);
                                    }

                                    // Collect part
                                    // Simple aggregation: if text, append to last text part? Or just push all parts?
                                    // Gemini stream sends separate parts. We can just accumulate them.
                                    // Optimization: Merge adjacent text parts.

                                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                        if let Some(last) = content_parts.last_mut() {
                                            if last.get("text").is_some()
                                                && part.get("thought").is_none()
                                                && last.get("thought").is_none()
                                            {
                                                // Merge text
                                                if let Some(last_text) =
                                                    last.get_mut("text").and_then(|v| v.as_str())
                                                {
                                                    let new_text = format!("{}{}", last_text, text);
                                                    *last = json!({"text": new_text});
                                                    continue;
                                                }
                                            }
                                        }
                                        content_parts.push(part.clone());
                                    } else {
                                        // Other parts (images, thoughts, function calls), just push
                                        content_parts.push(part.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Construct final response
    collected_response["candidates"][0]["content"]["parts"] = json!(content_parts);
    if let Some(fr) = finish_reason {
        collected_response["candidates"][0]["finishReason"] = json!(fr);
    }
    if let Some(usage) = usage_metadata {
        collected_response["usageMetadata"] = usage;
    }

    if let Some(feedback) = prompt_feedback {
        collected_response["promptFeedback"] = feedback;
    }
    if !response_has_visible_content(&collected_response)
        && !response_is_refusal(&collected_response)
    {
        return Err("Upstream ended without an answer or tool call".to_string());
    }
    Ok(collected_response)
}

/// 判断响应是否包含正文、工具调用或媒体；独立思考内容不视为最终答案。
pub(crate) fn response_has_visible_content(response: &Value) -> bool {
    response
        .get("candidates")
        .and_then(Value::as_array)
        .is_some_and(|candidates| {
            candidates.iter().any(|candidate| {
                candidate
                    .get("content")
                    .and_then(|content| content.get("parts"))
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                                return false;
                            }
                            part.get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.trim().is_empty())
                                || part.get("functionCall").is_some_and(Value::is_object)
                                || part.get("executableCode").is_some_and(Value::is_object)
                                || part
                                    .get("codeExecutionResult")
                                    .is_some_and(Value::is_object)
                                || part.get("toolCall").is_some_and(Value::is_object)
                                || part.get("toolResponse").is_some_and(Value::is_object)
                                || part
                                    .get("inlineData")
                                    .and_then(|data| data.get("data"))
                                    .and_then(Value::as_str)
                                    .is_some_and(|data| !data.is_empty())
                                || part
                                    .get("fileData")
                                    .and_then(|data| data.get("fileUri"))
                                    .and_then(Value::as_str)
                                    .is_some_and(|uri| !uri.is_empty())
                        })
                    })
            })
        })
}

/// 保留上游明确拒绝的结果，避免将其作为空响应自动重试。
pub(crate) fn response_is_refusal(response: &Value) -> bool {
    response
        .get("promptFeedback")
        .and_then(|feedback| feedback.get("blockReason"))
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.is_empty() && reason != "BLOCK_REASON_UNSPECIFIED")
        || response
            .get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| {
                candidates.iter().any(|candidate| {
                    matches!(
                        candidate.get("finishReason").and_then(Value::as_str),
                        Some(
                            "SAFETY"
                                | "RECITATION"
                                | "BLOCKLIST"
                                | "PROHIBITED_CONTENT"
                                | "SPII"
                                | "IMAGE_SAFETY"
                        )
                    )
                })
            })
}

#[cfg(test)]
mod compat_tests {
    use super::*;

    #[tokio::test]
    async fn compat_collector_preserves_fragmented_utf8_and_terminal_usage() {
        let raw = format!(
            "data: {}\n\ndata: {}",
            json!({"candidates": [{"content": {"parts": [{"text": "完整回复"}]}, "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 10}}),
            json!({"usageMetadata": {"candidatesTokenCount": 4, "totalTokenCount": 14}})
        );
        let chunks: Vec<_> = raw
            .as_bytes()
            .chunks(5)
            .map(|bytes| Ok::<_, String>(Bytes::copy_from_slice(bytes)))
            .collect();
        let response = collect_stream_to_json(futures::stream::iter(chunks), "compat-collector")
            .await
            .unwrap();
        assert_eq!(
            response["candidates"][0]["content"]["parts"][0]["text"],
            "完整回复"
        );
        assert_eq!(response["usageMetadata"]["promptTokenCount"], 10);
        assert_eq!(response["usageMetadata"]["candidatesTokenCount"], 4);
    }

    #[tokio::test]
    async fn compat_collector_rejects_thinking_only_and_error_frames() {
        for payload in [
            json!({"candidates": [{"content": {"parts": [{"thought": true, "text": "Considering"}]}, "finishReason": "STOP"}]}),
            json!({"error": {"code": 429, "message": "limited"}}),
            json!({"usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 0}}),
        ] {
            let input = futures::stream::iter([Ok::<_, String>(Bytes::from(format!(
                "data: {payload}\n\n"
            )))]);
            assert!(collect_stream_to_json(input, "compat-collector")
                .await
                .is_err());
        }
    }

    #[tokio::test]
    async fn compat_collector_preserves_explicit_refusal() {
        let input = futures::stream::iter([Ok::<_, String>(Bytes::from(
            "data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}",
        ))]);
        let response = collect_stream_to_json(input, "compat-collector")
            .await
            .unwrap();
        assert_eq!(response["promptFeedback"]["blockReason"], "SAFETY");
    }

    #[test]
    fn compat_visible_output_accepts_tools_and_media_but_not_thoughts() {
        for part in [
            json!({"functionCall": {"name": "tool"}}),
            json!({"inlineData": {"data": "image"}}),
            json!({"executableCode": {"code": "1 + 1"}}),
            json!({"codeExecutionResult": {"output": "2"}}),
            json!({"toolCall": {"id": "call-1"}}),
            json!({"toolResponse": {"id": "call-1"}}),
        ] {
            assert!(response_has_visible_content(
                &json!({"candidates": [{"content": {"parts": [part]}}]})
            ));
        }
        assert!(!response_has_visible_content(
            &json!({"candidates": [{"content": {"parts": [{"thought": true, "text": "thinking"}]}}]})
        ));
    }
}
