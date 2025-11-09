/* tests/src/main.rs */

mod build;

fn main() {
	println!("Building binary for testing...");
	println!(""); // Blank line for separation

	match build::builder::run() {
		Ok(_) => {
			println!("\nBinary built successfully.");
			// TODO: Add subsequent test execution steps here.
		}
		Err(e) => {
			eprintln!("\nFailed to build the binary. Error: {}", e);
			std::process::exit(1);
		}
	}
}
