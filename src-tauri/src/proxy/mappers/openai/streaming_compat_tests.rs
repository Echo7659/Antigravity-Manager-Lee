use super::streaming::create_openai_sse_stream;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};

async fn collect(frames: Vec<Bytes>) -> Vec<Value> {
    let input = futures::stream::iter(frames.into_iter().map(Ok::<_, String>));
    let output = create_openai_sse_stream(
        Box::pin(input),
        "gemini-3-flash".to_string(),
        "compat-stream-test".to_string(),
        1,
        None,
        true,
    );
    let chunks = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        output.collect::<Vec<_>>(),
    )
    .await
    .expect("stream must terminate");
    chunks
        .into_iter()
        .flat_map(|chunk| {
            let bytes = chunk.expect("stream chunk");
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter(|line| *line != "[DONE]")
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<Value>>()
        })
        .collect()
}

fn frame(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}

#[tokio::test]
async fn compat_stream_preserves_fragmented_terminal_frame_without_newline() {
    let raw = format!(
        "data: {}",
        json!({
            "response": {
                "candidates": [{"content": {"parts": [{"text": "末尾"}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2, "totalTokenCount": 5}
            }
        })
    );
    let chunks = raw
        .as_bytes()
        .chunks(7)
        .map(Bytes::copy_from_slice)
        .collect();
    let output = collect(chunks).await;
    assert!(output
        .iter()
        .any(|event| event["choices"][0]["delta"]["content"] == "末尾"));
    assert!(output
        .iter()
        .any(|event| event["usage"]["completion_tokens"] == 2));
    assert!(output.iter().all(|event| event.get("error").is_none()));
}

#[tokio::test]
async fn compat_stream_merges_partial_usage_after_finish() {
    let output = collect(vec![
        frame(json!({"usageMetadata": {"promptTokenCount": 100, "candidatesTokenCount": 0}})),
        frame(json!({
            "candidates": [{"content": {"parts": [{"text": "OK"}]}, "finishReason": "STOP"}],
            "usageMetadata": {"candidatesTokenCount": 20, "thoughtsTokenCount": 30, "totalTokenCount": 150}
        })),
        Bytes::from("data: {\"usageMetadata\":{\"promptTokenCount\":100}}"),
    ]).await;
    let usage = output
        .iter()
        .filter_map(|event| event.get("usage"))
        .last()
        .unwrap();
    assert_eq!(usage["prompt_tokens"], 100);
    assert_eq!(usage["completion_tokens"], 50);
    assert_eq!(usage["total_tokens"], 150);
}

#[tokio::test]
async fn compat_stream_reports_thinking_only_completion_as_incomplete() {
    let output = collect(vec![frame(json!({
        "candidates": [{"content": {"parts": [{"thought": true, "text": "Considering"}]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 0, "thoughtsTokenCount": 7, "totalTokenCount": 17}
    }))]).await;
    let failure = output
        .iter()
        .find(|event| event["error"]["code"] == "empty_response")
        .unwrap();
    assert_eq!(failure["usage"]["completion_tokens"], 7);
    assert!(output
        .iter()
        .all(|event| event["choices"][0]["finish_reason"] != "stop"));
}

#[tokio::test]
async fn compat_stream_accepts_tool_only_completion() {
    let output = collect(vec![frame(json!({
        "candidates": [{"content": {"parts": [{"functionCall": {"name": "get_temperature", "args": {"city": "Test"}}}]}, "finishReason": "STOP"}]
    }))]).await;
    assert!(output
        .iter()
        .any(|event| event["choices"][0]["finish_reason"] == "tool_calls"));
    assert!(output.iter().all(|event| event.get("error").is_none()));
}

#[tokio::test]
async fn compat_stream_preserves_refusal_without_reclassifying_as_empty_failure() {
    for response in [
        json!({"candidates": [{"finishReason": "SAFETY"}]}),
        json!({"promptFeedback": {"blockReason": "SAFETY"}}),
    ] {
        let output = collect(vec![frame(response)]).await;
        assert!(output
            .iter()
            .any(|event| event["choices"][0]["finish_reason"] == "content_filter"));
        assert!(output.iter().all(|event| event.get("error").is_none()));
    }
}

#[tokio::test]
async fn compat_stream_preserves_upstream_error_code() {
    let output = collect(vec![frame(
        json!({"error": {"code": 429, "message": "limited"}}),
    )])
    .await;
    assert_eq!(
        output
            .iter()
            .filter(|event| event.get("error").is_some())
            .count(),
        1
    );
    assert!(output.iter().any(|event| event["error"]["code"] == 429));
}
