use std::{future::Future, time::Duration};

/// 响应头等待与响应体传输使用独立时限，避免正常长流被提前截断。
pub(crate) fn duration(model: Option<&str>) -> Duration {
    if model.is_some_and(|name| name.to_lowercase().contains("image")) {
        return Duration::from_secs(600);
    }
    let seconds = std::env::var("ABV_UPSTREAM_HEADER_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30)
        .clamp(1, 300);
    Duration::from_secs(seconds)
}

pub(crate) async fn wait<F>(
    future: F,
    duration: Duration,
) -> Result<F::Output, tokio::time::error::Elapsed>
where
    F: Future,
{
    tokio::time::timeout(duration, future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn compat_upstream_header_wait_stops_unresponsive_endpoint() {
        assert!(
            wait(std::future::pending::<()>(), Duration::from_millis(10))
                .await
                .is_err()
        );
        let result = wait(
            async { Err::<(), _>("connection reset") },
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(result.unwrap(), Err("connection reset"));
        assert_eq!(
            wait(async { 429 }, Duration::from_millis(10))
                .await
                .unwrap(),
            429
        );
    }

    #[test]
    fn compat_upstream_header_wait_preserves_image_generation_allowance() {
        assert_eq!(
            duration(Some("gemini-3.1-flash-image")),
            Duration::from_secs(600)
        );
    }
}
