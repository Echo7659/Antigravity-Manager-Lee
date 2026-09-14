use crate::proxy::{server::AppState, token_manager::TokenManager};
use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use futures::StreamExt;
use std::sync::Arc;

struct RefreshAfterResponse {
    manager: Arc<TokenManager>,
    email: String,
}
impl Drop for RefreshAfterResponse {
    fn drop(&mut self) {
        self.manager.schedule_quota_refresh(&self.email);
    }
}

/// 响应结束或客户端断开后刷新已使用账号的额度，不依赖请求日志开关。
pub async fn quota_refresh_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let response = next.run(request).await;
    wrap_quota_refresh(response, state.token_manager)
}

pub(crate) fn wrap_quota_refresh(response: Response, manager: Arc<TokenManager>) -> Response {
    let Some(email) = response
        .headers()
        .get("x-account-email")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return response;
    };
    let guard = RefreshAfterResponse { manager, email };
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _guard = guard;
        let mut body = body.into_data_stream();
        while let Some(chunk) = body.next().await { yield chunk; }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}
