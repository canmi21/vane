use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlowError {
	#[error("plugin not found: {name}")]
	PluginNotFound { name: String },

	#[error("branch `{branch}` not found in step `{step}`")]
	BranchNotFound { step: String, branch: String },

	#[error("flow execution timed out after {timeout:?}")]
	ExecutionTimeout { timeout: Duration },

	#[error("plugin `{name}` failed")]
	PluginFailed {
		name: String,
		#[source]
		source: anyhow::Error,
	},

	#[error("stream already consumed at step `{step}`")]
	StreamAlreadyConsumed { step: String },
}
