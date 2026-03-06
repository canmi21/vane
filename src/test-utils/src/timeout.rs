use std::future::Future;
use std::time::Duration;

/// Asserts that `fut` completes within the given duration. Returns the result.
/// Panics if the future does not complete in time.
pub async fn assert_within<F, T>(duration: Duration, fut: F) -> T
where
    F: Future<Output = T>,
{
    tokio::time::timeout(duration, fut)
        .await
        .unwrap_or_else(|_| panic!("operation did not complete within {duration:?}"))
}

/// Asserts that `fut` does NOT complete within the given duration.
/// Panics if the future completes before the timeout.
pub async fn assert_timeout<F, T>(duration: Duration, fut: F)
where
    F: Future<Output = T>,
{
    match tokio::time::timeout(duration, fut).await {
        Err(_) => {}
        Ok(_) => panic!("expected timeout after {duration:?} but operation completed"),
    }
}
