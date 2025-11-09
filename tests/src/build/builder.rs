/* tests/src/build/builder.rs */

use std::env;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

/// Executes `cargo install --path .` in the parent directory to build the main project binary.
///
/// This function streams the stdout and stderr from the cargo command to the
/// current process's console in real-time. It blocks until the build
/// process is complete.
///
/// # Returns
///
/// * `Ok(())` if the build process exits with a success code.
/// * `Err(io::Error)` if the command cannot be spawned or if the build fails.
pub fn run() -> io::Result<()> {
	let manifest_dir =
		env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR environment variable is not set");
	let project_root = PathBuf::from(manifest_dir)
		.parent()
		.expect("Failed to get parent directory of the test manifest")
		.to_path_buf();

	let mut command = Command::new("cargo");
	// Add `--color=always` to force colorized output
	command.args(["--color=always", "install", "--path", "."]);
	command.current_dir(&project_root);

	command.stdout(Stdio::piped());
	command.stderr(Stdio::piped());

	let mut child = command.spawn()?;

	let stdout = child
		.stdout
		.take()
		.expect("Failed to capture stdout from child process");
	let stderr = child
		.stderr
		.take()
		.expect("Failed to capture stderr from child process");

	let stdout_thread = thread::spawn(move || {
		let reader = BufReader::new(stdout);
		for line in reader.lines() {
			match line {
				Ok(line) => println!("{}", line),
				Err(e) => eprintln!("Error reading stdout: {}", e),
			}
		}
	});

	let stderr_thread = thread::spawn(move || {
		let reader = BufReader::new(stderr);
		for line in reader.lines() {
			match line {
				Ok(line) => eprintln!("{}", line),
				Err(e) => eprintln!("Error reading stderr: {}", e),
			}
		}
	});

	stdout_thread
		.join()
		.expect("The stdout handler thread has panicked");
	stderr_thread
		.join()
		.expect("The stderr handler thread has panicked");

	let status = child.wait()?;

	if status.success() {
		Ok(())
	} else {
		Err(io::Error::new(
			io::ErrorKind::Other,
			format!("Build process failed with exit code: {}", status),
		))
	}
}
