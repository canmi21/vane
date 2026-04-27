//! Config loading entry point.
//!
//! See `spec/architecture/09-config.md`. The MVP scope of this module:
//!
//! 1. Best-effort `dotenvy` load of `<config_dir>/.env`. **OS env wins**
//!    — `dotenvy::from_path` does not override pre-existing keys, which
//!    matches operator expectations (systemd / supervisor unit files
//!    are authoritative).
//! 2. Scan `<config_dir>/rules/*.json` for [`RawRuleFile`]s.
//! 3. Read every `VANE_*` deployment constant into a typed [`Env`]
//!    snapshot.
//!
//! Out of MVP scope (not parsed yet): `<config_dir>/config.json`. The
//! global daemon settings file (listeners, management, wasm pool config
//! per `09-config.md` § _Top-level file schema_) is the daemon's own
//! startup concern — its schema is still in flux through S2. Today it
//! is silently ignored.
//!
//! Feature: S1-26 + S1-26a.

mod env;
mod loader;

pub use env::{Env, EnvReader, ProcessEnv};
pub use loader::scan_rules_dir;

use std::path::Path;

use crate::compile::merge::RawRuleFile;
use crate::error::Error;

/// Result of [`load`]: rule files (unmerged) plus the typed `Env`
/// snapshot. Downstream callers thread `files` into
/// [`crate::compile::compile`] and read `env` for deployment constants.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
	pub files: Vec<RawRuleFile>,
	pub env: Env,
}

/// Load a vane config directory.
///
/// Order of operations:
///
/// 1. If `<config_dir>/.env` exists, run `dotenvy::from_path` on it.
///    **Pre-existing OS env keys win** — operators who set values via
///    systemd / `EnvironmentFile=` / docker `-e` flag override what's
///    in `.env`. A missing `.env` is not an error; many deployments
///    rely entirely on OS-level env.
/// 2. Scan `<config_dir>/rules/*.json` via [`scan_rules_dir`].
/// 3. Read `VANE_*` deployment constants into [`Env`].
///
/// # Errors
/// - `<config_dir>/rules/` does not exist or is not a directory
///   (propagated from [`scan_rules_dir`]).
/// - Any `.json` under `rules/` fails to parse as `RawRuleFile`.
/// - Any `VANE_*` env var has an invalid value (non-integer, not
///   `"0"`/`"1"` for booleans, malformed `SocketAddr`, etc.).
///
/// **Not** an error:
/// - `.env` file is missing.
/// - `<config_dir>/config.json` is missing or malformed (it is not
///   parsed at this stage).
pub fn load(config_dir: &Path) -> Result<LoadedConfig, Error> {
	let env_path = config_dir.join(".env");
	if env_path.is_file() {
		// `.env` is an optional operator override — a missing file is
		// normal and must not produce noise. The `.is_file()` guard
		// handles the common case; the `NotFound` arm below covers the
		// race where the file disappears between the guard and the open.
		// Other failures (malformed syntax, permission denied) are real
		// problems the operator should see.
		match dotenvy::from_path(&env_path) {
			Ok(()) => {}
			Err(dotenvy::Error::Io(ref io_err)) if io_err.kind() == std::io::ErrorKind::NotFound => {}
			Err(e) => {
				tracing::warn!(
					path = %env_path.display(),
					error = %e,
					".env parse failed; using OS env only",
				);
			}
		}
	}

	let rules_dir = config_dir.join("rules");
	let files = scan_rules_dir(&rules_dir)?;
	let env = Env::from_process_env()?;

	Ok(LoadedConfig { files, env })
}
