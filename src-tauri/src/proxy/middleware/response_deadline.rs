use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::{future::Future, time::Duration};

/// 文本生成在响应交付前受总时限约束；已交付的流和图像生成不受此时限截断。
pub(crate) fn applies(method: &str, uri: &str, model: Option<&str>) -> bool {
    if method != "POST" || model.is_some_and(|name| name.to_lowercase().contains("image")) {
        return false;
    }
    let path = uri.split('?').next().unwrap_or(uri);
    matches!(
        path,
        "/v1/chat/completions" | "/v1/messages" | "/v1/responses" | "/v1/completions"
    ) || (path.starts_with("/v1beta/models/")
        && (path.ends_with(":generateContent") || path.ends_with(":streamGenerateContent")))
}

pub(crate) async fn run<F>(future: F, timeout: Duration) -> Response
where
    F: Future<Output = Response>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(response) => response,
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(serde_json::json!({"error": {
                "message": format!("Upstream did not produce a response within {} seconds", timeout.as_secs()),
                "type": "timeout_error",
                "code": "upstream_response_timeout"
            }})),
        ).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn compat_response_deadline_limits_only_text_generation() {
        assert!(applies(
            "POST",
            "/v1/chat/completions",
            Some("gemini-3.7-flash")
        ));
        assert!(applies(
            "POST",
            "/v1beta/models/gemini-pro:streamGenerateContent?alt=sse",
            Some("gemini-pro")
        ));
        assert!(!applies(
            "POST",
            "/v1/chat/completions",
            Some("gemini-3.1-flash-image")
        ));
        assert!(!applies("POST", "/api/accounts/refresh", None));
        assert!(!applies("GET", "/health", None));
    }

    #[tokio::test]
    async fn compat_response_deadline_cancels_waiting_handler() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Guard(dropped.clone());
        let response = run(
            async move {
                let _guard = guard;
                std::future::pending::<Response>().await
            },
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(dropped.load(Ordering::SeqCst));
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],
            "upstream_response_timeout"
        );
    }

    #[tokio::test]
    async fn compat_response_deadline_preserves_delivered_stream_and_errors() {
        let response = run(
            async {
                let body = Body::from_stream(futures::stream::once(async {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok::<_, std::io::Error>("data: OK\n\n")
                }));
                Response::new(body)
            },
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            "data: OK\n\n"
        );
        let response = run(
            async { StatusCode::TOO_MANY_REQUESTS.into_response() },
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
