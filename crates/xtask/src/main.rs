// xtask is the workspace's task-runner binary, invoked through the
// cargo alias `cargo xtask <subcommand>` (see .cargo/config.toml).
// Subcommands replace the perl scripts that used to live under
// scripts/, putting workspace-invariant logic into Rust where it
// gets type-checking, shared workspace deps, and `#[test]` coverage.

// CLI help (`///` on clap-derived enums) renders `VANE_BIN`,
// `CARGO_REGISTRY_TOKEN`, and similar identifiers as plain text;
// wrapping them in backticks would muddy the rendered --help.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod build_test_bins;
mod check_spec_anchors;
mod publish;
mod sync_deps;
mod workspace;

#[derive(Parser)]
#[command(name = "xtask", about = "vane workspace task runner")]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand)]
enum Command {
	/// Build the test binaries (vane, vaned) and write VANE_BIN / VANED_BIN to NEXTEST_ENV.
	BuildTestBins,
	/// Verify spec-anchor references in source comments resolve to real headings.
	CheckSpecAnchors,
	/// Reconcile workspace.dependencies versions with each crate's own version.
	#[command(subcommand)]
	SyncDeps(SyncDepsCmd),
	/// crates.io publish workflow: plan, dry-run, or real publish.
	#[command(subcommand)]
	Publish(PublishCmd),
}

#[derive(Subcommand)]
enum SyncDepsCmd {
	/// Exit non-zero on drift, listing stale workspace dep versions.
	Check,
	/// Rewrite the root Cargo.toml in place to bring versions in sync.
	Write,
}

#[derive(Subcommand)]
enum PublishCmd {
	/// Print the publish plan in topological order (table or JSON).
	Plan {
		/// Restrict the plan to a single crate.
		#[arg(long)]
		only: Option<String>,
		/// Emit newline-delimited JSON instead of a table.
		#[arg(long)]
		json: bool,
	},
	/// Dry-run cargo publish for every plan row.
	Dry {
		/// Restrict the dry-run to a single crate.
		#[arg(long)]
		only: Option<String>,
	},
	/// Real cargo publish for every plan row (requires CARGO_REGISTRY_TOKEN).
	Run {
		/// Restrict the publish to a single crate.
		#[arg(long)]
		only: Option<String>,
		/// Skip the `just gate` pre-flight.
		#[arg(long)]
		skip_gate: bool,
	},
}

// xtask's mutually-exclusive crypto-backend split mirrors vane-engine.
// The `compile_error!` pair below mirrors engine/src/lib.rs § crypto
// so a stray `--features "aws-lc-rs,ring"` (or `--no-default-features`
// without a replacement) fails at compile time, not at the first
// `ClientConfig` build.
#[cfg(all(feature = "aws-lc-rs", feature = "ring"))]
compile_error!("`aws-lc-rs` and `ring` are mutually exclusive crypto backends — pick exactly one.");

#[cfg(not(any(feature = "aws-lc-rs", feature = "ring")))]
compile_error!(
	"one of `aws-lc-rs` or `ring` must be enabled — the default features include `aws-lc-rs`."
);

fn main() -> Result<()> {
	// xtask uses rustls (via ureq) for the publish workflow's HTTPS
	// requests to the crates.io sparse index. rustls 0.23 panics if no
	// crypto provider is installed when a `ClientConfig` is built, and
	// ureq is configured with `rustls-no-provider` so we install the
	// workspace's chosen provider here. Idempotent: a second call is
	// silently ignored, matching engine::crypto::install_default_provider.
	#[cfg(feature = "aws-lc-rs")]
	let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
	#[cfg(all(feature = "ring", not(feature = "aws-lc-rs")))]
	let _ = rustls::crypto::ring::default_provider().install_default();

	match Cli::parse().command {
		Command::BuildTestBins => build_test_bins::run(),
		Command::CheckSpecAnchors => check_spec_anchors::run(),
		Command::SyncDeps(SyncDepsCmd::Check) => sync_deps::run(sync_deps::Mode::Check),
		Command::SyncDeps(SyncDepsCmd::Write) => sync_deps::run(sync_deps::Mode::Write),
		Command::Publish(PublishCmd::Plan { only, json }) => publish::plan(only.as_deref(), json),
		Command::Publish(PublishCmd::Dry { only }) => {
			publish::run(publish::Mode::Dry, only.as_deref(), false)
		}
		Command::Publish(PublishCmd::Run { only, skip_gate }) => {
			publish::run(publish::Mode::Real, only.as_deref(), skip_gate)
		}
	}
}
