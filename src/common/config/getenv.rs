/* src/common/config/getenv.rs */

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

#[cfg(test)]
mod tests {
	use super::*;
	use serial_test::serial;
	use temp_env;

	/// Tests that get_env correctly retrieves a value when the environment variable is set.
	#[test]
	#[serial]
	fn test_get_env_retrieves_value() {
		temp_env::with_var("TEST_KEY_EXISTS", Some("my_value"), || {
			assert_eq!(
				get_env("TEST_KEY_EXISTS", "default".to_string()),
				"my_value"
			);
		});
	}

	/// Tests that get_env falls back to the default value when the variable is not set.
	#[test]
	#[serial]
	fn test_get_env_uses_default() {
		temp_env::with_var_unset("TEST_KEY_UNSET", || {
			assert_eq!(
				get_env("TEST_KEY_UNSET", "default_value".to_string()),
				"default_value"
			);
		});
	}

	/// Tests the string to_lowercase conversion utility.
	#[test]
	fn test_to_lowercase_converts_correctly() {
		assert_eq!(to_lowercase("Hello World"), "hello world");
		assert_eq!(to_lowercase("ALREADY_LOWER"), "already_lower");
		assert_eq!(to_lowercase("123!@#"), "123!@#");
	}

	/// Tests the numeric string validation utility.
	#[test]
	fn test_is_numeric_validates_correctly() {
		assert!(is_numeric("12345"));
		assert!(!is_numeric("123a45"));
		assert!(!is_numeric("123 45"));
		assert!(is_numeric("")); // An empty string is considered numeric by this logic.
	}
}
