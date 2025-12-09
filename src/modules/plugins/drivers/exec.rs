/* src/modules/plugins/drivers/exec.rs */

use crate::modules::plugins::model::{MiddlewareOutput, ResolvedInputs};
use anyhow::{Result, anyhow};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub async fn execute(
	program: &str,
	args: &[String],
	env: &HashMap<String, String>,
	inputs: ResolvedInputs,
) -> Result<MiddlewareOutput> {
	log(
		LogLevel::Debug,
		&format!("➜ Spawning external command: {} {:?}", program, args),
	);

	let mut cmd = Command::new(program);
	cmd.args(args);
	cmd.envs(env);
	cmd.stdin(Stdio::piped());
	cmd.stdout(Stdio::piped());
	// Refactor: Capture stderr to pipe instead of inheriting directly to terminal.
	// This allows us to wrap plugin logs with Vane's logging system.
	cmd.stderr(Stdio::piped());

	let mut child = cmd
		.spawn()
		.map_err(|e| anyhow!("Failed to spawn plugin process: {}", e))?;

	// Write inputs to Stdin
	let mut input_payload = serde_json::to_vec(&inputs)?;
	// Fix: Append a newline to ensure line-based readers (like 'read' in shell)
	// detect the input correctly. JSON ignores this whitespace.
	input_payload.push(b'\n');

	if let Some(mut stdin) = child.stdin.take() {
		stdin
			.write_all(&input_payload)
			.await
			.map_err(|e| anyhow!("Failed to write to plugin stdin: {}", e))?;
	}

	// Wait for output (captures stdout and stderr)
	let output = child
		.wait_with_output()
		.await
		.map_err(|e| anyhow!("Plugin process failed during execution: {}", e))?;

	// Refactor: Process captured stderr and log as Debug level.
	// This hides plugin internal logs during normal operation unless LOG_LEVEL=debug.
	if !output.stderr.is_empty() {
		let stderr_output = String::from_utf8_lossy(&output.stderr);
		for line in stderr_output.lines() {
			if !line.trim().is_empty() {
				log(LogLevel::Debug, &format!("{}", line));
			}
		}
	}

	if !output.status.success() {
		return Err(anyhow!(
			"Plugin process exited with error status: {}",
			output.status
		));
	}

	// Parse Stdout as MiddlewareOutput
	let result: MiddlewareOutput = serde_json::from_slice(&output.stdout)
		.map_err(|e| anyhow!("Failed to parse plugin output JSON: {}", e))?;

	Ok(result)
}
