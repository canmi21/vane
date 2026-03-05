/* src/main.rs */

use std::env;

pub mod bootstrap;
pub mod lazycert;

#[tokio::main]
#[allow(clippy::vec_init_then_push)]
async fn main() {
	// Handle command-line version argument.
	if let Some(arg) = env::args().nth(1)
		&& (arg == "-v" || arg == "--version")
	{
		println!(
			"{} {} ({} {})",
			env!("CARGO_PKG_NAME"),
			env!("CARGO_PKG_VERSION"),
			env!("GIT_COMMIT_SHORT"),
			env!("BUILD_DATE")
		);

		let mut features = Vec::new();
		#[cfg(feature = "tcp")]
		features.push("tcp");
		#[cfg(feature = "udp")]
		features.push("udp");
		#[cfg(feature = "tls")]
		features.push("tls");
		#[cfg(feature = "quic")]
		features.push("quic");
		#[cfg(feature = "httpx")]
		features.push("httpx");
		#[cfg(feature = "domain-target")]
		features.push("domain-target");
		#[cfg(feature = "console")]
		features.push("console");
		#[cfg(feature = "h2upstream")]
		features.push("h2upstream");
		#[cfg(feature = "h3upstream")]
		features.push("h3upstream");
		#[cfg(feature = "cgi")]
		features.push("cgi");
		#[cfg(feature = "static")]
		features.push("static");
		#[cfg(feature = "ratelimit")]
		features.push("ratelimit");

		let features_str = if features.is_empty() {
			"none".to_owned()
		} else {
			features.join(", ")
		};
		println!("features: [{features_str}]");

		return; // Exit after printing version.
	}

	// If no version arg, start the vane proxy server.
	bootstrap::startup::start().await;
}
