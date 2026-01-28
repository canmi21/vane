/* src/lazycert/config.rs */

use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LazyCertConfig {
	/// Enable LazyCert integration
	#[serde(default)]
	pub enabled: bool,

	/// LazyCert API URL
	#[validate(url)]
	pub url: String,

	/// API access token
	#[validate(length(min = 1, message = "token cannot be empty"))]
	pub token: String,

	/// Challenge poll interval in seconds
	#[serde(default = "default_poll_interval")]
	#[validate(range(min = 1, max = 300))]
	pub poll_interval: u64,

	/// Self-reported public IP ("auto" or explicit IP)
	#[serde(default = "default_public_ip")]
	pub public_ip: String,
}

fn default_poll_interval() -> u64 {
	5
}

fn default_public_ip() -> String {
	"auto".to_owned()
}
