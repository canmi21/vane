use tracing_subscriber::EnvFilter;

/// Initializes tracing for tests with `--nocapture`-friendly output.
///
/// Respects `RUST_LOG` env var; defaults to `debug` level.
/// Safe to call multiple times — only the first call takes effect.
pub fn init_tracing() {
	let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "debug".parse().unwrap());
	let _ = tracing_subscriber::fmt().with_test_writer().with_env_filter(filter).try_init();
}
