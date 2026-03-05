pub mod exec;
pub mod httpx;
#[cfg(unix)]
pub mod unix;

use anyhow::Result;
use vane_engine::engine::interfaces::{ExternalPluginDriver, MiddlewareOutput, ResolvedInputs};

/// Executes the appropriate driver logic based on the configuration.
pub async fn execute_driver(
	driver: &ExternalPluginDriver,
	name: &str,
	inputs: ResolvedInputs,
) -> Result<MiddlewareOutput> {
	match driver {
		ExternalPluginDriver::Http { url } => httpx::execute(url, name, inputs).await,
		#[cfg(unix)]
		ExternalPluginDriver::Unix { path } => unix::execute(path, name, inputs).await,
		#[cfg(not(unix))]
		ExternalPluginDriver::Unix { .. } => {
			anyhow::bail!(
				"Unix socket ipc call are not supported on windows platform (requested by plugin: {})",
				name
			)
		}
		ExternalPluginDriver::Command { program, args, env } => {
			exec::execute(program, args, env, inputs).await
		}
	}
}
