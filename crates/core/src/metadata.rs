use crate::error::Error;
use crate::fetch::{FetchKind, FetchOutputModes, FetchPhase};
use crate::middleware::MiddlewareKind;
use crate::wasm_runtime::PluginExport;

pub struct MiddlewareMetadata {
	pub kind: MiddlewareKind,
	pub stateless: bool,
	pub needs_body: bool,
	pub validate_args: fn(&serde_json::Value) -> Result<(), Error>,
}

/// Pass-through validator for plugin-backed middleware. Per-plugin
/// arg schemas live inside the WASM module itself (the export's
/// `validate-args` host call); the compile pipeline only needs
/// `Some(meta)` to confirm the name resolves. Schema violations
/// surface at link time when the runtime invokes the plugin.
fn plugin_validate_args_pass(_: &serde_json::Value) -> Result<(), Error> {
	Ok(())
}

impl MiddlewareMetadata {
	/// Build a middleware-metadata record from a plugin export. Used by
	/// the daemon-side metadata provider to satisfy compile-stage
	/// queries for `<module>:<export>` references — the plugin's
	/// `kind` / `stateless` / `needs_body` map directly onto the
	/// middleware-metadata shape; `inspects` is plugin-internal and
	/// has no analogue here.
	#[must_use]
	pub fn from_plugin(export: &PluginExport) -> Self {
		Self {
			kind: export.kind,
			stateless: export.stateless,
			needs_body: export.needs_body,
			validate_args: plugin_validate_args_pass,
		}
	}
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

#[cfg(test)]
mod tests {
	use serde_json::{Value, json};

	use super::*;
	use crate::error::Error;

	fn reject_null_accept_object(v: &Value) -> Result<(), Error> {
		match v {
			Value::Object(_) => Ok(()),
			_ => Err(Error::compile("expected object")),
		}
	}

	struct StaticMwProvider;
	impl MiddlewareMetadataProvider for StaticMwProvider {
		fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
			if name == "rate_limit" {
				Some(MiddlewareMetadata {
					kind: MiddlewareKind::L7Request,
					stateless: false,
					needs_body: false,
					validate_args: reject_null_accept_object,
				})
			} else {
				None
			}
		}
	}

	struct StaticFetchProvider;
	impl FetchMetadataProvider for StaticFetchProvider {
		fn get(&self, kind: FetchKind) -> Option<FetchMetadata> {
			if kind == FetchKind::HttpProxy {
				Some(FetchMetadata {
					kind: FetchKind::HttpProxy,
					phase: FetchPhase::L7,
					output_modes: FetchOutputModes { response: true, tunnel: false },
					validate_args: reject_null_accept_object,
				})
			} else {
				None
			}
		}
	}

	#[test]
	fn middleware_provider_returns_known_record_and_none_for_unknown() {
		let p = StaticMwProvider;
		let meta = p.get("rate_limit").expect("known entry");
		assert_eq!(meta.kind, MiddlewareKind::L7Request);
		assert!(!meta.stateless);
		assert!(!meta.needs_body);
		assert!(p.get("no_such_middleware").is_none());
	}

	#[test]
	fn middleware_validate_args_fn_pointer_dispatches() {
		let p = StaticMwProvider;
		let meta = p.get("rate_limit").expect("known entry");
		assert!((meta.validate_args)(&Value::Null).is_err());
		assert!((meta.validate_args)(&json!({ "rate": 100 })).is_ok());
	}

	#[test]
	fn middleware_provider_is_object_safe() {
		let p: &dyn MiddlewareMetadataProvider = &StaticMwProvider;
		assert!(p.get("rate_limit").is_some());
		assert!(p.get("unknown").is_none());
	}

	#[test]
	fn fetch_provider_returns_known_kind_and_none_for_unknown() {
		let p = StaticFetchProvider;
		let meta = p.get(FetchKind::HttpProxy).expect("known kind");
		assert_eq!(meta.kind, FetchKind::HttpProxy);
		assert_eq!(meta.phase, FetchPhase::L7);
		assert_eq!(meta.output_modes, FetchOutputModes { response: true, tunnel: false });
		assert!(p.get(FetchKind::L4Forward).is_none());
	}

	#[test]
	fn fetch_validate_args_fn_pointer_dispatches() {
		let p = StaticFetchProvider;
		let meta = p.get(FetchKind::HttpProxy).expect("known kind");
		assert!((meta.validate_args)(&Value::Null).is_err());
		assert!((meta.validate_args)(&json!({ "upstream": "127.0.0.1:80" })).is_ok());
	}

	#[test]
	fn fetch_provider_is_object_safe() {
		let p: &dyn FetchMetadataProvider = &StaticFetchProvider;
		assert!(p.get(FetchKind::HttpProxy).is_some());
		assert!(p.get(FetchKind::WebSocketUpgrade).is_none());
	}

	#[test]
	fn middleware_metadata_from_plugin_copies_fields() {
		let export = PluginExport {
			name: "jwt-validator".to_string(),
			kind: MiddlewareKind::L7Request,
			stateless: false,
			needs_body: true,
			inspects: vec!["http.header.authorization".to_string()],
		};
		let meta = MiddlewareMetadata::from_plugin(&export);
		assert_eq!(meta.kind, MiddlewareKind::L7Request);
		assert!(!meta.stateless);
		assert!(meta.needs_body);
		// Plugin-backed validate_args is a pass-through — schema lives
		// in the WASM module, not in the metadata record.
		assert!((meta.validate_args)(&Value::Null).is_ok());
		assert!((meta.validate_args)(&json!({ "skew": 30 })).is_ok());
	}
}
