/* modules/totp/examples/verify.rs */

use totp::{current_unix_time, generate_combined_token, verify_combined_token};

fn main() {
	let seeds = ["a", "b", "c", "d", "e", "f"];
	let time = current_unix_time();
	let window = 15;
	let token = generate_combined_token(seeds, time, window);
	println!("Generated: {}", token);
	let ok = verify_combined_token(seeds, time, &token, window, 2, "s");
	println!("Verified: {}", ok);
}
