//! Management wire format: line-delimited JSON over a duplex byte
//! stream. Each request is one JSON object on one line; each response
//! is one JSON object on one line; lines end with `\n`. No length
//! prefix — the framing is the newline. NDJSON keeps tools such as
//! `nc -U` piped through `jq` usable for ad-hoc poking. The same frame
//! shapes ride the HTTP-over-TCP transport (NDJSON over chunked
//! encoding).
//!
//! See [`spec/crates/mgmt.md`](../../../spec/crates/mgmt.md).

use serde::{Deserialize, Serialize};

/// Client → server frame.
///
/// `id` is client-assigned and echoed by the server's response so a
/// future multiplexed transport can interleave concurrent requests on
/// one socket. The current Unix implementation serialises
/// request/response per-connection; the wire shape doesn't depend on
/// that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
	pub id: u64,
	pub verb: String,
	#[serde(default)]
	pub args: serde_json::Value,
}

/// Server → client frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
	pub id: u64,
	#[serde(flatten)]
	pub outcome: ResponseOutcome,
}

/// Successful result or structured error. Flattened into `Response`
/// so the wire shape is `{"id":N,"result":{...}}` or
/// `{"id":N,"error":{...}}` rather than a nested `outcome` key.
///
/// Streaming verbs use the additional `Event` and `End` variants:
///
/// - `Event { event }` — one frame in a streaming response. The server
///   may emit zero or more of these per request.
/// - `End {}` — terminates a streaming response normally. Encoded as
///   `{"id":N,"end":{}}` so the client can match on the field name.
///
/// `#[serde(untagged)]` collapses each variant to its single field —
/// the keys (`result`, `error`, `event`, `end`) are mutually
/// exclusive, so the discriminator is the field name itself rather
/// than a separate `"kind"` tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseOutcome {
	Result { result: serde_json::Value },
	Error { error: WireError },
	Event { event: serde_json::Value },
	End { end: EndMarker },
}

/// Empty marker payload for the `End` outcome. Encoded as `{}` so
/// future fields (e.g. a final summary) can be added without breaking
/// the wire shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndMarker {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireError {
	pub kind: WireErrorKind,
	pub message: String,
}

/// Error category. The full string message carries detail; the kind is
/// the machine-readable discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireErrorKind {
	UnknownVerb,
	BadArgs,
	Internal,
	/// Future-proof for streaming verbs and other deferred capabilities.
	NotImplemented,
}

/// Encode a value as JSON and append `\n`. Centralises framing so
/// server.rs / client.rs share one implementation.
///
/// # Errors
/// Returns the underlying [`serde_json::Error`] if `value` fails to
/// serialize.
pub fn encode_line<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
	let mut buf = serde_json::to_vec(value)?;
	buf.push(b'\n');
	Ok(buf)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn request_round_trips_through_json_with_args() {
		let req =
			Request { id: 42, verb: "stats".to_string(), args: serde_json::json!({ "scope": "all" }) };
		let encoded = serde_json::to_string(&req).expect("serialize");
		let decoded: Request = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded.id, 42);
		assert_eq!(decoded.verb, "stats");
		assert_eq!(decoded.args, serde_json::json!({ "scope": "all" }));
	}

	#[test]
	fn request_default_args_are_null() {
		// Args are optional on the wire; missing key decodes as Value::Null.
		let raw = r#"{"id":1,"verb":"ping"}"#;
		let req: Request = serde_json::from_str(raw).expect("deserialize");
		assert!(req.args.is_null());
	}

	#[test]
	fn response_result_serializes_with_flat_result_key() {
		let resp = Response {
			id: 7,
			outcome: ResponseOutcome::Result { result: serde_json::json!({ "pong": true }) },
		};
		let value = serde_json::to_value(&resp).expect("to_value");
		assert_eq!(value["id"], 7);
		assert_eq!(value["result"], serde_json::json!({ "pong": true }));
		assert!(value.get("error").is_none(), "result frame must not carry error key");
		assert!(value.get("outcome").is_none(), "must flatten — no nested outcome key");
	}

	#[test]
	fn response_error_serializes_with_flat_error_key() {
		let resp = Response {
			id: 3,
			outcome: ResponseOutcome::Error {
				error: WireError { kind: WireErrorKind::UnknownVerb, message: "no such verb".to_string() },
			},
		};
		let value = serde_json::to_value(&resp).expect("to_value");
		assert_eq!(value["id"], 3);
		assert_eq!(value["error"]["kind"], "unknown_verb");
		assert_eq!(value["error"]["message"], "no such verb");
		assert!(value.get("result").is_none());
	}

	#[test]
	fn unknown_verb_kind_round_trips_via_snake_case() {
		for kind in [
			WireErrorKind::UnknownVerb,
			WireErrorKind::BadArgs,
			WireErrorKind::Internal,
			WireErrorKind::NotImplemented,
		] {
			let s = serde_json::to_string(&kind).expect("serialize kind");
			let back: WireErrorKind = serde_json::from_str(&s).expect("deserialize kind");
			assert_eq!(kind, back);
		}
		assert_eq!(serde_json::to_string(&WireErrorKind::UnknownVerb).unwrap(), "\"unknown_verb\"");
		assert_eq!(serde_json::to_string(&WireErrorKind::BadArgs).unwrap(), "\"bad_args\"");
		assert_eq!(
			serde_json::to_string(&WireErrorKind::NotImplemented).unwrap(),
			"\"not_implemented\""
		);
	}

	#[test]
	fn response_event_outcome_serializes_with_event_key() {
		let resp = Response {
			id: 9,
			outcome: ResponseOutcome::Event { event: serde_json::json!({ "kind": "trajectory" }) },
		};
		let value = serde_json::to_value(&resp).expect("to_value");
		assert_eq!(value["id"], 9);
		assert_eq!(value["event"]["kind"], "trajectory");
		assert!(value.get("result").is_none());
		assert!(value.get("error").is_none());
		assert!(value.get("end").is_none());
	}

	#[test]
	fn response_end_outcome_serializes_as_empty_end_object() {
		let resp = Response { id: 4, outcome: ResponseOutcome::End { end: EndMarker {} } };
		let value = serde_json::to_value(&resp).expect("to_value");
		assert_eq!(value["id"], 4);
		assert_eq!(value["end"], serde_json::json!({}));
		assert!(value.get("event").is_none());
	}

	#[test]
	fn response_event_round_trips_through_json() {
		// Round-trip a few mixed outcomes to confirm the untagged enum
		// disambiguates by the field name (`result` / `error` / `event`
		// / `end`) and not by ordering.
		let frames = vec![
			Response { id: 1, outcome: ResponseOutcome::Result { result: serde_json::json!(42) } },
			Response { id: 2, outcome: ResponseOutcome::Event { event: serde_json::json!("hi") } },
			Response { id: 3, outcome: ResponseOutcome::End { end: EndMarker {} } },
		];
		for f in frames {
			let s = serde_json::to_string(&f).expect("serialize");
			let back: Response = serde_json::from_str(&s).expect("deserialize");
			assert_eq!(back.id, f.id);
			match (&f.outcome, &back.outcome) {
				(ResponseOutcome::Result { .. }, ResponseOutcome::Result { .. })
				| (ResponseOutcome::Event { .. }, ResponseOutcome::Event { .. })
				| (ResponseOutcome::End { .. }, ResponseOutcome::End { .. }) => {}
				other => panic!("variant changed: {other:?}"),
			}
		}
	}

	#[test]
	fn encode_line_appends_newline() {
		let req = Request { id: 1, verb: "ping".to_string(), args: serde_json::Value::Null };
		let bytes = encode_line(&req).expect("encode");
		assert_eq!(*bytes.last().expect("non-empty"), b'\n');
		// Body before the newline must be valid JSON of the same shape.
		let body = &bytes[..bytes.len() - 1];
		let decoded: Request = serde_json::from_slice(body).expect("decode body");
		assert_eq!(decoded.verb, "ping");
	}
}
