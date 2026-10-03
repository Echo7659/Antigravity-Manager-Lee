//! 响应流只校验结果，不发起额外请求；账号重试由 handler 的受限循环负责。

use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::pin::Pin;

use crate::proxy::mappers::gemini::collector::{response_has_visible_content, response_is_refusal};

/// 保留真实响应及用量，阻止无正文、工具或拒绝结果的流报告成功。
pub fn guard_response_stream<S, E>(
    mut stream: Pin<Box<S>>,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + ?Sized + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Box::pin(async_stream::stream! {
        let mut buffer = BytesMut::new();
        let mut has_output = false;
        let mut has_refusal = false;
        loop {
            match stream.next().await {
                Some(Ok(bytes)) => buffer.extend_from_slice(&bytes),
                Some(Err(error)) => {
                    yield Err(error.to_string());
                    return;
                }
                None if !buffer.is_empty() => buffer.extend_from_slice(b"\n"),
                None => break,
            }
            while let Some(pos) = buffer.iter().position(|&byte| byte == b'\n') {
                let line = buffer.split_to(pos + 1).freeze();
                let Ok(text) = std::str::from_utf8(&line) else {
                    yield Ok(line);
                    continue;
                };
                let Some(data) = text.trim().strip_prefix("data: ") else {
                    yield Ok(line);
                    continue;
                };
                if data == "[DONE]" {
                    if has_output || has_refusal {
                        yield Ok(line);
                    }
                    continue;
                }
                let Ok(mut frame) = serde_json::from_str::<Value>(data) else {
                    yield Ok(line);
                    continue;
                };
                let response = if frame.get("response").is_some() {
                    &mut frame["response"]
                } else {
                    &mut frame
                };
                has_output |= response_has_visible_content(response);
                has_refusal |= response_is_refusal(response);
                // 保留 parts、usageMetadata 和尾随用量；只移除虚假的成功终止标记。
                if !has_output && !has_refusal {
                    if let Some(candidates) = response.get_mut("candidates").and_then(Value::as_array_mut) {
                        for candidate in candidates {
                            if let Some(candidate) = candidate.as_object_mut() {
                                candidate.remove("finishReason");
                            }
                        }
                    }
                }
                yield Ok(Bytes::from(format!("data: {frame}\n")));
            }
        }
        if !has_output && !has_refusal {
            yield Err("Upstream ended without an answer or tool call (empty_response)".to_string());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thinking_frame() -> Value {
        json!({
            "response": {
                "candidates": [{"content": {"parts": [{"thought": true, "text": "思考"}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 11, "thoughtsTokenCount": 7, "totalTokenCount": 18}
            }
        })
    }

    async fn collect(frames: Vec<Result<Bytes, String>>) -> (String, Vec<String>) {
        let chunks = guard_response_stream(Box::pin(futures::stream::iter(frames)))
            .collect::<Vec<_>>()
            .await;
        let mut output = String::new();
        let mut errors = Vec::new();
        for chunk in chunks {
            match chunk {
                Ok(bytes) => output.push_str(std::str::from_utf8(&bytes).unwrap()),
                Err(error) => errors.push(error),
            }
        }
        (output, errors)
    }

    #[tokio::test]
    async fn compat_guard_empty_stream_fails_before_output() {
        let (output, errors) = collect(vec![]).await;
        assert!(output.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("empty_response"));
    }

    #[tokio::test]
    async fn compat_guard_preserves_thinking_usage_without_continuation() {
        let raw = format!(
            "data: {}\n\ndata: {{\"usageMetadata\":{{\"totalTokenCount\":18}}}}\n\ndata: [DONE]",
            thinking_frame()
        );
        let frames = raw
            .as_bytes()
            .chunks(5)
            .map(|part| Ok(Bytes::copy_from_slice(part)))
            .collect();
        let (output, errors) = collect(frames).await;
        assert!(output.contains("\"promptTokenCount\":11"));
        assert!(output.contains("\"thoughtsTokenCount\":7"));
        assert_eq!(output.matches("\"totalTokenCount\":18").count(), 2);
        assert!(!output.contains("finishReason"));
        assert!(!output.contains("[DONE]"));
        assert!(!output.contains("task ready"));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("empty_response"));
    }

    #[tokio::test]
    async fn compat_guard_preserves_usage_before_success_or_transport_failure() {
        for succeeds in [true, false] {
            let first = Ok(Bytes::from(format!("data: {}\n\n", thinking_frame())));
            let next = if succeeds {
                Ok(Bytes::from("data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\"}]},\"finishReason\":\"STOP\"}]}\n\n"))
            } else {
                Err("connection reset".to_string())
            };
            let (output, errors) = collect(vec![first, next]).await;
            assert!(output.contains("\"promptTokenCount\":11"));
            assert!(output.contains("\"thoughtsTokenCount\":7"));
            assert_eq!(errors.is_empty(), succeeds);
            assert_eq!(output.contains("answer"), succeeds);
            if !succeeds {
                assert_eq!(errors, ["connection reset"]);
                assert!(!output.contains("STOP"));
            }
        }
    }

    #[tokio::test]
    async fn compat_guard_protocol_errors_retain_thinking_usage() {
        use crate::proxy::mappers::{claude, openai::streaming};
        for protocol in ["claude", "chat", "legacy", "responses"] {
            for transport_error in [false, true] {
                let session = format!("guard-{protocol}-{transport_error}");
                let mut frames = vec![
                    Ok(Bytes::from("data: {\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"thinking\"}]}}]}\n\n")),
                    Ok(Bytes::from("data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":11,\"thoughtsTokenCount\":7,\"candidatesTokenCount\":0,\"totalTokenCount\":18}}\n\n")),
                ];
                if transport_error {
                    frames.push(Err("connection reset".to_string()));
                }
                let guarded = guard_response_stream(Box::pin(futures::stream::iter(frames)));
                let mapped = match protocol {
                    "claude" => claude::create_claude_sse_stream(
                        guarded,
                        session.clone(),
                        "test@example.com".into(),
                        Some(session),
                        false,
                        1_000_000,
                        None,
                        1,
                        None,
                        vec![],
                    ),
                    "chat" => streaming::create_openai_sse_stream(
                        guarded,
                        "gemini-test".into(),
                        session,
                        1,
                        None,
                        false,
                    ),
                    "legacy" => streaming::create_legacy_sse_stream(
                        guarded,
                        "gemini-test".into(),
                        session,
                        1,
                    ),
                    _ => streaming::create_codex_sse_stream(
                        guarded,
                        "gemini-test".into(),
                        session.clone(),
                        1,
                        0,
                        session,
                        None,
                        false,
                    ),
                };
                let output = mapped
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
                    .collect::<String>();
                let values: Vec<Value> = output
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter_map(|data| serde_json::from_str(data).ok())
                    .collect();
                let usages: Vec<&Value> = values
                    .iter()
                    .filter_map(|value| {
                        value
                            .get("usage")
                            .or_else(|| value.pointer("/response/usage"))
                    })
                    .collect();
                assert!(
                    usages.iter().any(|usage| usage
                        .get("input_tokens")
                        .or_else(|| usage.get("prompt_tokens"))
                        == Some(&json!(11))
                        && usage
                            .get("output_tokens")
                            .or_else(|| usage.get("completion_tokens"))
                            == Some(&json!(7))),
                    "{protocol}: {output}"
                );
                assert!(output.contains("\"error\""), "{protocol}: {output}");
                assert!(!output.contains("task ready"));
                assert!(!output.contains("Recovered by Antigravity"));
                assert!(!output.contains("\"finish_reason\":\"stop\""));
                assert!(!output.contains("event: message_stop"));
                assert!(!output.contains("event: response.completed"));
                if protocol == "responses" {
                    assert!(output.contains("event: response.failed"), "{output}");
                }
            }
        }
    }

    #[tokio::test]
    async fn compat_guard_accepts_tool_calls_and_refusals() {
        for candidate in [
            json!({"content": {"parts": [{"functionCall": {"name": "lookup", "args": {}}}]}, "finishReason": "STOP"}),
            json!({"finishReason": "SAFETY"}),
        ] {
            let (output, errors) = collect(vec![Ok(Bytes::from(format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"candidates": [candidate]})
            )))])
            .await;
            assert!(errors.is_empty());
            assert!(output.contains("finishReason"));
            assert!(output.contains("[DONE]"));
        }
    }
}
