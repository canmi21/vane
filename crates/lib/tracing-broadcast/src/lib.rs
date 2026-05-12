//! [`BroadcastTracingLayer`] — a `tracing_subscriber::Layer` that fans
//! every emitted [`tracing::Event`] into a `tokio::sync::broadcast`
//! channel as a [`TracingFrame`] (timestamp / level / target / message
//! / structured fields).
//!
//! The layer composes alongside the host's normal subscriber stack
//! (`tracing_subscriber::fmt::Layer` writing to stderr is unaffected);
//! it adds one more sink without changing user-visible logging. Each
//! subscriber gets its own `broadcast::Receiver` with independent
//! backlog tracking; slow subscribers see `RecvError::Lagged(n)` and
//! resume from the next available frame, which the operator-facing
//! transport can surface as a sentinel.
//!
//! `TracingFrame` derives `serde::Serialize` / `Deserialize`, so the
//! transport (NDJSON, websocket text frames, JSON-RPC, …) is just
//! `serde_json::to_string(&frame)`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Default broadcast channel capacity for tracing frames. Tracing
/// events are lower-volume than flow logs (one per log line vs one
/// per per-step trajectory), so the channel is sized smaller —
/// `spec/flow-model.md` § _Flow log verbosity_ owns the per-stream
/// sizing rationale. Override via `VANE_TRACE_BROADCAST_CAP`.
///
/// A subscriber that falls more than `capacity` events behind sees
/// [`broadcast::error::RecvError::Lagged`] and resumes from the next
/// available event.
pub const DEFAULT_BROADCAST_CAP: usize = 1024;

/// Env var that overrides [`DEFAULT_BROADCAST_CAP`] at construction
/// time. Values that fail to parse or evaluate to 0 fall back to the
/// default — same shape as the rest of the daemon's `VANE_*` knobs.
pub const ENV_BROADCAST_CAP: &str = "VANE_TRACE_BROADCAST_CAP";

fn resolve_broadcast_cap() -> usize {
	std::env::var(ENV_BROADCAST_CAP)
		.ok()
		.and_then(|s| s.parse::<usize>().ok())
		.filter(|&n| n > 0)
		.unwrap_or(DEFAULT_BROADCAST_CAP)
}

/// Wire shape for a single tracing event.
///
/// Field layout follows JSON-formatter conventions (`t` / `level` /
/// `target` / `message` / `fields`) so `jq` queries written for
/// JSON-rendered logs apply to the broadcast stream unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingFrame {
	/// Wall-clock timestamp in milliseconds since the Unix epoch.
	pub t: u64,
	/// `tracing::Level` rendered as upper-case (`ERROR` / `WARN` /
	/// `INFO` / `DEBUG` / `TRACE`).
	pub level: String,
	/// `metadata.target()` — typically the module path that emitted
	/// the event.
	pub target: String,
	/// Formatted `message` field if present, otherwise empty.
	pub message: String,
	/// Remaining structured fields as `{name → JSON value}`.
	pub fields: serde_json::Value,
}

/// Tracing layer that broadcasts each event as a [`TracingFrame`].
///
/// Cheap to clone — the inner [`broadcast::Sender`] is itself `Clone`
/// and shares the channel through an internal `Arc`. Compose one
/// instance into the subscriber registry and keep cloned references
/// wherever handlers need to call [`Self::subscribe`].
#[derive(Clone)]
pub struct BroadcastTracingLayer {
	tx: broadcast::Sender<TracingFrame>,
}

impl BroadcastTracingLayer {
	#[must_use]
	pub fn new() -> Self {
		Self::with_capacity(resolve_broadcast_cap())
	}

	/// Explicit-capacity constructor for tests and bespoke wiring. The
	/// `new` / `Default` path resolves capacity from
	/// `VANE_TRACE_BROADCAST_CAP`, falling back to
	/// [`DEFAULT_BROADCAST_CAP`].
	#[must_use]
	pub fn with_capacity(capacity: usize) -> Self {
		// Initial receiver dropped immediately; subscribers come and go
		// over the lifetime of the process as clients connect.
		let cap = capacity.max(1);
		let (tx, _initial_rx) = broadcast::channel(cap);
		Self { tx }
	}

	/// Subscribe to live events. Each subscriber gets its own receiver
	/// with independent backlog tracking.
	#[must_use]
	pub fn subscribe(&self) -> broadcast::Receiver<TracingFrame> {
		self.tx.subscribe()
	}

	/// Active subscriber count — exposed for tests and instrumentation.
	#[must_use]
	pub fn subscriber_count(&self) -> usize {
		self.tx.receiver_count()
	}
}

impl Default for BroadcastTracingLayer {
	fn default() -> Self {
		Self::new()
	}
}

impl<S> Layer<S> for BroadcastTracingLayer
where
	S: Subscriber,
{
	fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
		let metadata = event.metadata();
		let mut visitor = FieldVisitor::default();
		event.record(&mut visitor);

		let frame = TracingFrame {
			t: SystemTime::now()
				.duration_since(UNIX_EPOCH)
				.map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
			level: metadata.level().to_string(),
			target: metadata.target().to_string(),
			message: visitor.message.unwrap_or_default(),
			fields: serde_json::Value::Object(visitor.fields),
		};
		// `send` returns `Err` only when there are no receivers — that's
		// the steady state when no client is tailing. Drop quietly.
		let _ = self.tx.send(frame);
	}
}

/// Field visitor that splits the special `message` field from the rest
/// of the event's structured fields. Numeric and boolean values stay
/// typed in JSON; anything else (including `Debug`-only values) is
/// stringified — operators can still grep on the rendered form, and
/// the lossy degradation matches what `tracing-subscriber::fmt` does.
#[derive(Default)]
struct FieldVisitor {
	message: Option<String>,
	fields: serde_json::Map<String, serde_json::Value>,
}

impl FieldVisitor {
	fn record(&mut self, field: &Field, value: serde_json::Value) {
		if field.name() == "message"
			&& let serde_json::Value::String(s) = &value
		{
			self.message = Some(s.clone());
			return;
		}
		self.fields.insert(field.name().to_string(), value);
	}
}

impl Visit for FieldVisitor {
	fn record_str(&mut self, field: &Field, value: &str) {
		self.record(field, serde_json::Value::String(value.to_string()));
	}

	fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
		// Most `tracing::info!("…", foo)` calls hit this path because
		// the macro records the message via `Debug`. We strip the outer
		// quotes that `{value:?}` adds for string-typed values so the
		// rendered message matches the raw `info!` argument.
		let s = format!("{value:?}");
		let trimmed = if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
			s[1..s.len() - 1].to_string()
		} else {
			s
		};
		self.record(field, serde_json::Value::String(trimmed));
	}

	fn record_i64(&mut self, field: &Field, value: i64) {
		self.record(field, serde_json::Value::Number(value.into()));
	}

	fn record_u64(&mut self, field: &Field, value: u64) {
		self.record(field, serde_json::Value::Number(value.into()));
	}

	fn record_bool(&mut self, field: &Field, value: bool) {
		self.record(field, serde_json::Value::Bool(value));
	}

	fn record_f64(&mut self, field: &Field, value: f64) {
		// `serde_json::Number::from_f64` returns `Option` — non-finite
		// floats fall back to a string representation.
		let v = serde_json::Number::from_f64(value)
			.map_or_else(|| serde_json::Value::String(value.to_string()), serde_json::Value::Number);
		self.record(field, v);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tracing_subscriber::layer::SubscriberExt;
	use tracing_subscriber::util::SubscriberInitExt;

	fn install(layer: BroadcastTracingLayer) -> tracing::subscriber::DefaultGuard {
		// Per-test subscriber so concurrent tests don't share a global
		// dispatcher. `set_default` returns a guard that restores the
		// previous dispatcher on drop.
		tracing_subscriber::registry().with(layer).set_default()
	}

	#[tokio::test]
	async fn broadcast_tracing_layer_emits_event_to_subscriber() {
		let layer = BroadcastTracingLayer::new();
		let mut rx = layer.subscribe();
		assert_eq!(layer.subscriber_count(), 1);

		let _guard = install(layer.clone());
		tracing::info!(addr = "127.0.0.1", port = 8080_u64, "listener bound");

		let frame = rx.recv().await.expect("recv frame");
		assert_eq!(frame.level, "INFO");
		assert_eq!(frame.message, "listener bound");
		assert_eq!(frame.fields["addr"], "127.0.0.1");
		assert_eq!(frame.fields["port"], 8080);
		assert!(!frame.target.is_empty(), "target captured from metadata");
	}

	#[tokio::test]
	async fn broadcast_tracing_layer_no_receivers_silently_drops() {
		// With zero subscribers, broadcast::send returns Err. The layer
		// must not propagate that — `on_event` is on the tracing hot path.
		let layer = BroadcastTracingLayer::new();
		assert_eq!(layer.subscriber_count(), 0);
		let _guard = install(layer.clone());
		// No panic, no deadlock.
		tracing::warn!("no subscribers attached");
	}

	#[tokio::test]
	async fn broadcast_tracing_layer_lagged_subscriber_sees_recv_error() {
		let layer = BroadcastTracingLayer::new();
		let mut rx = layer.subscribe();
		let _guard = install(layer.clone());

		// Saturate the channel beyond a single subscriber's buffer
		// without ever calling `rx.recv` so the backlog overflows.
		for i in 0..(DEFAULT_BROADCAST_CAP + 5) {
			tracing::info!(seq = i as u64, "saturate");
		}
		match rx.recv().await {
			Err(broadcast::error::RecvError::Lagged(n)) => {
				assert!(n >= 5, "expected lag >= 5, got {n}");
			}
			other => panic!("expected Lagged, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn broadcast_tracing_layer_preserves_typed_int_field() {
		let layer = BroadcastTracingLayer::new();
		let mut rx = layer.subscribe();
		let _guard = install(layer.clone());
		tracing::info!(count = 42_i64, ratio = 0.5_f64, ok = true, "typed");
		let frame = rx.recv().await.expect("recv");
		assert_eq!(frame.fields["count"], 42);
		assert_eq!(frame.fields["ok"], true);
		assert!(frame.fields["ratio"].is_number());
	}
}
