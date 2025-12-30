/* src/modules/plugins/drivers/exec.rs */

use crate::modules::plugins::model::{MiddlewareOutput, ResolvedInputs};
use anyhow::Result;
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

	let mut child = match cmd.spawn() {
		Ok(c) => c,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to spawn plugin process '{}': {}", program, e),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// Write inputs to Stdin
	let mut input_payload = serde_json::to_vec(&inputs)?;
	input_payload.push(b'\n');

	if let Some(mut stdin) = child.stdin.take() {
		if let Err(e) = stdin.write_all(&input_payload).await {
			log(
				LogLevel::Error,
				&format!("✗ Failed to write to plugin stdin: {}", e),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	}

	// Wait for output (captures stdout and stderr)
	let output = match child.wait_with_output().await {
		Ok(o) => o,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Plugin process '{}' failed: {}", program, e),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// Refactor: Process captured stderr and log as Debug level.
	if !output.stderr.is_empty() {
		let stderr_output = String::from_utf8_lossy(&output.stderr);
		for line in stderr_output.lines() {
			if !line.trim().is_empty() {
				log(LogLevel::Debug, &format!("{}", line));
			}
		}
	}

	if !output.status.success() {
		log(
			LogLevel::Error,
			&format!(
				"✗ Plugin process '{}' exited with error status: {}",
				program, output.status
			),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}

	// Parse Stdout as MiddlewareOutput
	let result: MiddlewareOutput = match serde_json::from_slice(&output.stdout) {
		Ok(r) => r,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Failed to parse output JSON from plugin '{}': {}",
					program, e
				),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	Ok(result)
}
