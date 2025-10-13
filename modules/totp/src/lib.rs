/* modules/totp/src/lib.rs */

mod totp;
pub use totp::{current_unix_time, generate_combined_token, verify_combined_token};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn wasm_generate_combined_token(seeds: Vec<String>, time: u64, window: u64) -> String {
	let mut arr = ["", "", "", "", "", ""];
	for (i, s) in seeds.iter().enumerate().take(6) {
		arr[i] = s;
	}
	generate_combined_token(arr, time, window)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn wasm_verify_combined_token(
	seeds: Vec<String>,
	time: u64,
	token: String,
	window: u64,
	allowed_windows: u32,
	unit: String,
) -> bool {
	let mut arr = ["", "", "", "", "", ""];
	for (i, s) in seeds.iter().enumerate().take(6) {
		arr[i] = s;
	}
	verify_combined_token(arr, time, &token, window, allowed_windows, &unit)
}
