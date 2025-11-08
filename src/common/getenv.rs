/* src/common/getenv.rs */

use std::env;
use std::ffi::OsStr;

/// Gets an environment variable, returning a default value if it's not set.
pub fn get_env<T: AsRef<OsStr>>(key: T, default: String) -> String {
	env::var(key).unwrap_or(default)
}

/// Converts a string to lowercase.
pub fn to_lowercase(s: &str) -> String {
	s.to_lowercase()
}

/// Checks if a string contains only numeric characters.
pub fn is_numeric(s: &str) -> bool {
	s.chars().all(char::is_numeric)
}
