use crate::proxy::server::AppState;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

pub async fn service_status_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    // Always allow Admin API, internal endpoints and Auth callback
    if is_status_exempt(path) {
        return next.run(request).await;
    }

    let running = {
        let r = state.is_running.read().await;
        *r
    };

    if !running {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "Proxy service is currently disabled".to_string(),
        )
            .into_response();
    }

    next.run(request).await
}

fn is_status_exempt(path: &str) -> bool {
    path.starts_with("/api/")
        || path.starts_with("/internal/")
        || path == "/auth/callback"
        || path == "/health"
}

#[cfg(test)]
mod tests {
    use super::is_status_exempt;

    #[test]
    fn admin_models_remain_available_when_proxy_is_stopped() {
        assert!(is_status_exempt("/api/proxy/models"));
        assert!(!is_status_exempt("/v1/models"));
    }
}
