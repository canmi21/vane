/* src/main.rs */

use std::env;

pub mod common;
pub mod core;
pub mod middleware;
pub mod modules;

#[tokio::main]
async fn main() {
	// Handle command-line version argument.
	if let Some(arg) = env::args().nth(1) {
		if arg == "-v" || arg == "--version" {
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

	// If no version arg, start the vane proxy server.
	core::bootstrap::start().await;
}
