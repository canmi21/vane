/* src/common/sys/hotswap.rs */

use fancy_log::{LogLevel, log};
use std::future::Future;
use tokio::sync::mpsc;

/// Generic watch loop for hot-reloading configurations.
///
/// # Parameters
/// - `rx`: The receiver channel for filesystem events.
/// - `name`: The display name of the component being watched (e.g., "Application").
/// - `on_reload`: Async closure to execute when a change is detected.
pub async fn watch_loop<F, Fut>(mut rx: mpsc::Receiver<()>, name: &str, mut on_reload: F)
where
	F: FnMut() -> Fut,
	Fut: Future<Output = ()>,
{
	while rx.recv().await.is_some() {
		log(LogLevel::Info, &format!("➜ Config change signal received for {name}, reloading..."));
		on_reload().await;
	}
}
