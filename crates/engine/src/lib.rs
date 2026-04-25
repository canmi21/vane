//! vane runtime engine: executor, listeners, pools, TLS, built-in middleware.
//!
//! See `spec/architecture/02-flow.md`, `06-l4.md`, `07-l7.md`, `08-tls.md`, `13-rate-limit.md`.

#[cfg(all(feature = "aws-lc-rs", feature = "ring"))]
compile_error!("`aws-lc-rs` and `ring` features are mutually exclusive — pick one");

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!("one of `aws-lc-rs` or `ring` must be enabled");

pub mod executor;
pub mod factories;
pub mod fetch;
pub mod flow_graph;
pub mod flow_log_sink;
pub mod hot_reload;
pub mod listener;
pub mod middleware;
pub mod preset;
pub mod protocol_detect;
pub mod security;
pub mod terminator;
pub mod tracing_init;
pub mod upgrade;
pub mod verbosity;

pub use listener::ListenerSet;
pub use verbosity::VerbosityState;

pub mod crypto {
	pub const BACKEND_NAME: &str = {
		#[cfg(feature = "aws-lc-rs")]
		{
			"aws-lc-rs"
		}
		#[cfg(feature = "ring")]
		{
			"ring"
		}
	};
}

/// The stable feature-name list that `FlowGraph::link` copies into
/// `FlowGraphMeta::feature_set`. Order is documentation-stable so the
/// management API's `get_active_config` verb can diff snapshots across
/// boots. The crypto backend name always leads; optional features fold
/// in behind it via `#[cfg(feature = ...)]`.
pub const ENGINE_FEATURE_SET: &[&str] = &[
	crypto::BACKEND_NAME,
	#[cfg(feature = "h3")]
	"h3",
	#[cfg(feature = "cgi")]
	"cgi",
	#[cfg(feature = "acme")]
	"acme",
	#[cfg(feature = "acme-dns-cloudflare")]
	"acme-dns-cloudflare",
];
