use crate::error::Error;
use crate::fetch::{FetchKind, FetchOutputModes, FetchPhase};
use crate::middleware::MiddlewareKind;

pub struct MiddlewareMetadata {
	pub kind: MiddlewareKind,
	pub stateless: bool,
	pub needs_body: bool,
	pub validate_args: fn(&serde_json::Value) -> Result<(), Error>,
}

pub trait MiddlewareMetadataProvider {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata>;
}

pub struct FetchMetadata {
	pub kind: FetchKind,
	pub phase: FetchPhase,
	pub output_modes: FetchOutputModes,
	pub validate_args: fn(&serde_json::Value) -> Result<(), Error>,
}

pub trait FetchMetadataProvider {
	fn get(&self, kind: FetchKind) -> Option<FetchMetadata>;
}
