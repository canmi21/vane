use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::predicate::Predicate;

pub type ListenSpec = String;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RawRule {
	pub name: String,
	pub listen: Vec<ListenSpec>,
	#[serde(default, rename = "match")]
	pub match_predicate: Option<Predicate>,
	#[serde(default)]
	pub middleware_chain: Vec<MiddlewareRef>,
	#[serde(default)]
	pub fetch: Option<FetchSpec>,
	pub terminate: TerminatorSpec,
	#[serde(default)]
	pub source: SourceInfo,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MiddlewareRef {
	#[serde(rename = "use")]
	pub name: String,
	#[serde(default)]
	pub args: serde_json::Value,
	#[serde(default)]
	pub on_error: Option<OnErrorSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum OnErrorSpec {
	Close,
	Response(SynthResponse),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SynthResponse {
	pub status: u16,
	#[serde(default)]
	pub headers: Option<BTreeMap<String, String>>,
	#[serde(default)]
	pub body: Option<String>,
}

impl<'de> serde::Deserialize<'de> for OnErrorSpec {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		#[derive(serde::Deserialize)]
		#[serde(untagged)]
		enum Raw {
			Literal(String),
			Response { response: SynthResponse },
		}
		match Raw::deserialize(de)? {
			Raw::Literal(s) if s == "close" => Ok(Self::Close),
			Raw::Literal(other) => Err(serde::de::Error::unknown_variant(&other, &["close"])),
			Raw::Response { response } => Ok(Self::Response(response)),
		}
	}
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct FetchSpec {
	#[serde(rename = "type")]
	pub kind: String,
	#[serde(default)]
	pub args: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TerminatorSpec {
	#[serde(rename = "type")]
	pub kind: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct SourceInfo {
	#[serde(default)]
	pub file: PathBuf,
	#[serde(default)]
	pub line: u32,
}
