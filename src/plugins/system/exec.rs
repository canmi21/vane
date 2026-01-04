/* src/plugins/system/exec.rs */

use crate::common::config::getenv;
use crate::engine::contract::{MiddlewareOutput, ResolvedInputs};
use crate::plugins::core::external;
use anyhow::Result;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

pub async fn execute(
	program: &str,
	args: &[String],
	env: &HashMap<String, String>,
	inputs: ResolvedInputs,
) -> Result<MiddlewareOutput> {
	// SEC-2: Enforce trusted bin root validation at runtime.
	let resolved_program = match external::validate_command_path(program).await {
		Ok(p) => p,
		Err(e) => {
			log(
				LogLevel::Error,
				&format!("✗ Security violation during plugin execution: {}", e),
			);
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	let timeout_secs = getenv::get_env("FLOW_EXECUTION_TIMEOUT_SECS", "10".to_string())
		.parse::<u64>()
		.unwrap_or(10);

	log(
		LogLevel::Debug,
		&format!(
			"➜ Spawning external command (timeout {}s): {} {:?}",
			timeout_secs,
			resolved_program.display(),
			args
		),
	);

	let mut cmd = Command::new(resolved_program);
	cmd.args(args);

	// SEC-3: Sanitize environment variables
	let allow_linker =
		getenv::get_env("ALLOW_EXTERNAL_LINKER_ENV", "false".to_string()).to_lowercase() == "true";
	let allow_runtime =
		getenv::get_env("ALLOW_EXTERNAL_RUNTIME_ENV", "false".to_string()).to_lowercase() == "true";
	let allow_shell =
		getenv::get_env("ALLOW_EXTERNAL_SHELL_ENV", "false".to_string()).to_lowercase() == "true";
	let allow_path_append =
		getenv::get_env("ALLOW_EXTERNAL_PATH_ENV_APPEND", "false".to_string()).to_lowercase() == "true";

	let mut sanitized_env = HashMap::new();

	for (key, value) in env {
		let k_up = key.to_uppercase();

		// Category 1: Linker (LD_*, DYLD_*, etc.)
		let is_linker = k_up.starts_with("LD_")
			|| k_up.starts_with("DYLD_")
			|| k_up.starts_with("_RLD_")
			|| k_up == "SHLIB_PATH"
			|| k_up == "LIBPATH";
		if is_linker {
			if allow_linker {
				sanitized_env.insert(key.clone(), value.clone());
			} else {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Security: Dropped Linker env var '{}' (ALLOW_EXTERNAL_LINKER_ENV is false)",
						key
					),
				);
			}
			continue;
		}

		// Category 2: Runtime (PYTHON*, NODE_*, etc.)
		let is_runtime = k_up.starts_with("PYTHON")
			|| k_up.starts_with("NODE_")
			|| k_up.starts_with("PERL")
			|| k_up.starts_with("RUBY")
			|| k_up.starts_with("JAVA_")
			|| k_up == "CLASSPATH";
		if is_runtime {
			if allow_runtime {
				sanitized_env.insert(key.clone(), value.clone());
			} else {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Security: Dropped Runtime env var '{}' (ALLOW_EXTERNAL_RUNTIME_ENV is false)",
						key
					),
				);
			}
			continue;
		}

		// Category 3: Shell (IFS, ENV, etc.)
		let is_shell = k_up == "IFS" || k_up == "ENV" || k_up == "BASH_ENV" || k_up == "SHELLOPTS";
		if is_shell {
			if allow_shell {
				sanitized_env.insert(key.clone(), value.clone());
			} else {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Security: Dropped Shell env var '{}' (ALLOW_EXTERNAL_SHELL_ENV is false)",
						key
					),
				);
			}
			continue;
		}

		// Category 4: Path
		if k_up == "PATH" {
			if allow_path_append {
				let system_path = std::env::var("PATH").unwrap_or_default();
				let separator = if cfg!(windows) { ";" } else { ":" };
				let new_path = format!("{}{}{}", system_path, separator, value);
				sanitized_env.insert(key.clone(), new_path);
				log(
					LogLevel::Debug,
					&format!(
						"⚙ Appended user PATH to system PATH for plugin '{}'",
						program
					),
				);
			} else {
				log(
					LogLevel::Warn,
					&format!(
						"⚠ Security: Dropped PATH env var from plugin '{}' (ALLOW_EXTERNAL_PATH_ENV_APPEND is false)",
						program
					),
				);
			}
			continue;
		}

		// Other variables: Pass through
		sanitized_env.insert(key.clone(), value.clone());
	}

	cmd.envs(sanitized_env);
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
			let _ = child.kill().await;
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	}

	// Wait for output (captures stdout and stderr) with timeout
	// We handle this manually because wait_with_output consumes the child object.
	let stdout = match child.stdout.take() {
		Some(s) => s,
		None => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to take stdout from plugin process '{}'", program),
			);
			let _ = child.kill().await;
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};
	let stderr = match child.stderr.take() {
		Some(s) => s,
		None => {
			log(
				LogLevel::Error,
				&format!("✗ Failed to take stderr from plugin process '{}'", program),
			);
			let _ = child.kill().await;
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	let mut stdout_res = Vec::new();
	let mut stderr_res = Vec::new();

	let mut stdout_reader = tokio::io::BufReader::new(stdout);
	let mut stderr_reader = tokio::io::BufReader::new(stderr);

	let process_future = async {
		let (out_res, err_res, status_res) = tokio::join!(
			tokio::io::AsyncReadExt::read_to_end(&mut stdout_reader, &mut stdout_res),
			tokio::io::AsyncReadExt::read_to_end(&mut stderr_reader, &mut stderr_res),
			child.wait()
		);
		out_res.and(err_res).and(status_res.map_err(|e| e.into()))
	};

	let exit_status = match timeout(Duration::from_secs(timeout_secs), process_future).await {
		Ok(res) => match res {
			Ok(s) => s,
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
		},
		Err(_) => {
			log(
				LogLevel::Error,
				&format!(
					"✗ Plugin process '{}' timed out after {}s. Killing child.",
					program, timeout_secs
				),
			);
			let _ = child.kill().await;
			return Ok(MiddlewareOutput {
				branch: "failure".into(),
				store: None,
			});
		}
	};

	// Refactor: Process captured stderr and log as Debug level.
	if !stderr_res.is_empty() {
		let stderr_output = String::from_utf8_lossy(&stderr_res);
		for line in stderr_output.lines() {
			if !line.trim().is_empty() {
				log(LogLevel::Debug, &format!("{}", line));
			}
		}
	}

	if !exit_status.success() {
		log(
			LogLevel::Error,
			&format!(
				"✗ Plugin process '{}' exited with error status: {}",
				program, exit_status
			),
		);
		return Ok(MiddlewareOutput {
			branch: "failure".into(),
			store: None,
		});
	}

	// Parse Stdout as MiddlewareOutput
	let result: MiddlewareOutput = match serde_json::from_slice(&stdout_res) {
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
