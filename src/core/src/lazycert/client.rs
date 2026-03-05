/* src/core/src/lazycert/client.rs */

use anyhow::{Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct LazyCertClient {
	base_url: String,
	token: String,
	client: Client,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
	status: String,
	data: Option<T>,
	message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeInfo {
	pub id: String,
	pub domain: String,
	pub r#type: String,
	pub token: String,
	pub key_authorization: String,
}

#[derive(Debug, Deserialize)]
struct ChallengesResponse {
	challenges: Vec<ChallengeInfo>,
}

#[derive(Debug, Deserialize)]
pub struct CertificateResponse {
	pub status: String,
	pub mode: String,
	pub certificate: Option<String>,
	pub private_key: Option<String>,
	pub message: Option<String>,
}

impl LazyCertClient {
	#[must_use]
	pub fn new(base_url: &str, token: String) -> Self {
		let client = Client::builder()
			.timeout(Duration::from_secs(30))
			.build()
			.expect("Failed to create HTTP client");

		Self {
			base_url: base_url.trim_end_matches('/').to_owned(),
			token,
			client,
		}
	}
	/// Check if LazyCert is reachable
	pub async fn health(&self) -> Result<bool> {
		let url = format!("{}/health", self.base_url);
		let resp = self.client.get(&url).send().await?;
		Ok(resp.status().is_success())
	}

	/// Get pending HTTP-01 challenges
	pub async fn get_challenges(&self) -> Result<Vec<ChallengeInfo>> {
		let url = format!("{}/challenges", self.base_url);
		let resp: ApiResponse<ChallengesResponse> = self
			.client
			.get(&url)
			.bearer_auth(&self.token)
			.send()
			.await?
			.json()
			.await?;

		if resp.status != "success" {
			return Err(anyhow!("Failed to get challenges: {:?}", resp.message));
		}

		Ok(resp.data.map(|d| d.challenges).unwrap_or_default())
	}

	/// Mark challenge as solved (Vane has prepared the response)
	pub async fn mark_challenge_solved(&self, challenge_id: &str) -> Result<()> {
		let url = format!("{}/challenges/{}/solved", self.base_url, challenge_id);
		let resp = self
			.client
			.post(&url)
			.bearer_auth(&self.token)
			.json(&serde_json::json!({}))
			.send()
			.await?;

		if !resp.status().is_success() {
			return Err(anyhow!(
				"Failed to mark challenge solved: {}",
				resp.status()
			));
		}

		Ok(())
	}

	/// Request a new certificate
	pub async fn request_certificate(
		&self,
		id: &str,
		domains: Vec<String>,
		client_ip: Option<String>,
		mode: Option<String>,
	) -> Result<CertificateResponse> {
		let url = format!("{}/certificates", self.base_url);

		#[derive(Serialize)]
		struct Request {
			id: String,
			domains: Vec<String>,
			#[serde(skip_serializing_if = "Option::is_none")]
			client_ip: Option<String>,
			#[serde(skip_serializing_if = "Option::is_none")]
			mode: Option<String>,
		}

		let req = Request {
			id: id.to_owned(),
			domains,
			client_ip,
			mode,
		};
		let resp: ApiResponse<CertificateResponse> = self
			.client
			.post(&url)
			.bearer_auth(&self.token)
			.json(&req)
			.send()
			.await?
			.json()
			.await?;

		resp.data.ok_or_else(|| anyhow!("No data in response"))
	}
}
