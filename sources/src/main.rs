/* sources/src/main.rs */

use std::env;

mod common;
mod daemon;
mod middleware;

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
	core::bootstrap::start().await;
}
