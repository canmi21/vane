/* modules/totp/src/totp.rs */

use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha1 = Hmac<Sha1>;

fn generate_token(seed: &str, time: u64, window: u64) -> u32 {
	let counter = time / window;
	let mut mac = HmacSha1::new_from_slice(seed.as_bytes()).unwrap();
	mac.update(&counter.to_be_bytes());
	let hash = mac.finalize().into_bytes();
	let offset = (hash[hash.len() - 1] & 0x0f) as usize;
	let code = ((u32::from(hash[offset]) & 0x7f) << 24)
		| ((u32::from(hash[offset + 1]) & 0xff) << 16)
		| ((u32::from(hash[offset + 2]) & 0xff) << 8)
		| (u32::from(hash[offset + 3]) & 0xff);
	code % 1_000_000
}

pub fn generate_combined_token(seeds: [&str; 6], time: u64, window: u64) -> String {
	let tokens: Vec<String> = seeds
		.iter()
		.map(|s| format!("{:06}", generate_token(s, time, window)))
		.collect();
	tokens.join("-")
}

pub fn verify_combined_token(
	seeds: [&str; 6],
	time: u64,
	token: &str,
	window: u64,
	allowed_windows: u32,
	unit: &str,
) -> bool {
	let delta = if unit == "s" { 1 } else { 1 };
	let steps = match allowed_windows {
		0 | 1 => vec![0],
		n => {
			let mut all = vec![0i64];
			for i in 1..n as i64 {
				all.push(i);
				all.push(-i);
			}
			all
		}
	};

	for step in steps {
		let t = if step == 0 {
			time
		} else {
			time.wrapping_add_signed(step * (window * delta) as i64)
		};
		let gen_token = generate_combined_token(seeds, t, window);
		if gen_token == token {
			return true;
		}
	}
	false
}

pub fn current_unix_time() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs()
}
