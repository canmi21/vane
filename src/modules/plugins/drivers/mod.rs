/* src/modules/plugins/drivers/mod.rs */

pub mod exec;
pub mod httpx;
pub mod unix;

use crate::modules::plugins::core::model::{
	ExternalPluginDriver, MiddlewareOutput, ResolvedInputs,
};
use anyhow::Result;

/// Executes the appropriate driver logic based on the configuration.
pub async fn execute_driver(
	driver: &ExternalPluginDriver,
	name: &str,
	inputs: ResolvedInputs,
) -> Result<MiddlewareOutput> {
	match driver {
		ExternalPluginDriver::Http { url } => httpx::execute(url, name, inputs).await,
		ExternalPluginDriver::Unix { path } => unix::execute(path, name, inputs).await,
		ExternalPluginDriver::Command { program, args, env } => {
			exec::execute(program, args, env, inputs).await
		}
	}
}
