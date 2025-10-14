/* engine/src/main.rs */

use std::env;

pub mod common;
pub mod config;
pub mod daemon;
pub mod middleware;
pub mod modules;
pub mod servers;

#[tokio::main]
async fn main() {
	// Handle command-line version argument.
	if let Some(arg) = env::args().nth(1) {
		if arg == "version" || arg == "--version" {
			println!(
				"{} {} ({} {})",
				env!("CARGO_PKG_NAME"),
				env!("CARGO_PKG_VERSION"),
				env!("GIT_COMMIT_SHORT"),
				env!("BUILD_DATE")
			);
			return; // Exit after printing version.
		}
	}

	// If no version arg, start the server.
	daemon::bootstrap::start().await;
}
