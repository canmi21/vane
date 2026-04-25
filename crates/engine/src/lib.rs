//! vane runtime engine: executor, listeners, pools, TLS, built-in middleware.
//!
//! See `spec/architecture/02-flow.md`, `06-l4.md`, `07-l7.md`, `08-tls.md`, `13-rate-limit.md`.

// Crypto backend is mutually exclusive by design — see
// spec/architecture/16-crate-layout.md § _Crypto backend_.
// `cargo … --all-features` deliberately does not work on this workspace;
// pick one backend explicitly (e.g. `--features "aws-lc-rs,h3,cgi"`).
#[cfg(all(feature = "aws-lc-rs", feature = "ring"))]
compile_error!(
	"`aws-lc-rs` and `ring` are mutually exclusive crypto backends — pick exactly one. \
	 If you ran `--all-features`, drop one explicitly: \
	 `--no-default-features --features \"aws-lc-rs,h3,cgi,acme,acme-dns-cloudflare\"` \
	 (or replace `aws-lc-rs` with `ring`). \
	 See spec/architecture/16-crate-layout.md § Crypto backend.",
);

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!(
	"one of `aws-lc-rs` or `ring` must be enabled — \
	 the default crate features include `aws-lc-rs`; if you set \
	 `--no-default-features`, add `--features aws-lc-rs` (or `ring` for \
	 32-bit / no-C-toolchain targets).",
);

pub(crate) mod body_adapter;
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
	// Each cfg branch is wired so exactly one is ever active per legal
	// build — `aws-lc-rs` wins when both crypto features are mistakenly
	// enabled, `ring` is its strict complement, and the `neither` arm is
	// a degenerate-case stub that's only there to keep this `const` block
	// well-typed (`&str`, not `()`) so the friendly `compile_error!`
	// above is the only error the user sees, with no E0308 collateral.
	// The stub string is unreachable: compilation aborts at the
	// compile_error before this constant is ever read.
	pub const BACKEND_NAME: &str = {
		#[cfg(feature = "aws-lc-rs")]
		{
			"aws-lc-rs"
		}
		#[cfg(all(feature = "ring", not(feature = "aws-lc-rs")))]
		{
			"ring"
		}
		#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
		{
			""
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
