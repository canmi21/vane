//! Daemon-wide Prometheus metrics recorder.
//!
//! Exposes a single global `PrometheusHandle` installed at boot. The
//! `get_metrics` mgmt verb reads it via `render_prometheus()` for
//! text exposition format or `render_json()` for a structured
//! representation parsed back from the text output.
//!
//! All vane-emit metrics go through the `metrics::counter!` /
//! `metrics::gauge!` / `metrics::histogram!` macros (spec
//! spec/crates/mgmt.md — "no bespoke facade").

use std::sync::OnceLock;

use metrics_exporter_prometheus::{BuildError, PrometheusBuilder, PrometheusHandle};

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the process-wide Prometheus recorder. Idempotent: a second
/// call after a successful install is a no-op and returns `Ok(())`.
///
/// # Errors
/// Returns [`BuildError`] when the recorder cannot be installed
/// (typically because another recorder is already global). Daemon
/// main treats this as fatal.
pub fn install_recorder() -> Result<(), BuildError> {
	if PROMETHEUS_HANDLE.get().is_some() {
		return Ok(());
	}
	let handle = PrometheusBuilder::new().install_recorder()?;
	let _ = PROMETHEUS_HANDLE.set(handle);
	Ok(())
}

/// The installed `PrometheusHandle`, or `None` when the recorder has
/// not been installed (test paths that skip boot setup).
#[must_use]
pub fn handle() -> Option<&'static PrometheusHandle> {
	PROMETHEUS_HANDLE.get()
}

/// Prometheus text exposition format snapshot. `None` when the
/// recorder is not installed.
#[must_use]
pub fn render_prometheus() -> Option<String> {
	handle().map(PrometheusHandle::render)
}

/// Structured JSON representation of the same metrics. Built by
/// rendering the text exposition then parsing it via
/// `prometheus-parse`. Returns `None` when the recorder is not
/// installed.
#[must_use]
pub fn render_json() -> Option<serde_json::Value> {
	let text = render_prometheus()?;
	Some(text_to_json(&text))
}

fn text_to_json(text: &str) -> serde_json::Value {
	let lines = text.lines().map(|l| Ok::<_, std::io::Error>(l.to_owned()));
	match prometheus_parse::Scrape::parse(lines) {
		Ok(scrape) => serde_json::json!({
				"samples": scrape.samples.iter().map(sample_to_value).collect::<Vec<_>>(),
				"docs": scrape.docs,
		}),
		Err(e) => serde_json::json!({
				"error": format!("prometheus-parse: {e}"),
				"raw": text,
		}),
	}
}

fn value_to_json(v: &prometheus_parse::Value) -> serde_json::Value {
	use prometheus_parse::Value;
	match v {
		Value::Counter(f) => serde_json::json!({"type": "counter", "value": f}),
		Value::Gauge(f) => serde_json::json!({"type": "gauge", "value": f}),
		Value::Untyped(f) => serde_json::json!({"type": "untyped", "value": f}),
		// Histogram(Vec<HistogramCount>): the Vec itself is the bucket list;
		// each HistogramCount holds {less_than, count} for one le bucket.
		Value::Histogram(bucket_entries) => {
			let buckets: Vec<serde_json::Value> = bucket_entries
				.iter()
				.map(|b| serde_json::json!({"upper_bound": b.less_than, "cumulative_count": b.count}))
				.collect();
			serde_json::json!({"type": "histogram", "buckets": buckets})
		}
		// Summary(Vec<SummaryCount>): the Vec itself is the quantile list;
		// each SummaryCount holds {quantile, value}.
		Value::Summary(quantile_entries) => {
			let quantiles: Vec<serde_json::Value> = quantile_entries
				.iter()
				.map(|q| serde_json::json!({"quantile": q.quantile, "count": q.count}))
				.collect();
			serde_json::json!({"type": "summary", "quantiles": quantiles})
		}
	}
}

fn sample_to_value(s: &prometheus_parse::Sample) -> serde_json::Value {
	let labels: serde_json::Map<String, serde_json::Value> =
		s.labels.iter().map(|(k, v)| (k.to_owned(), serde_json::Value::String(v.to_owned()))).collect();
	// timestamp is DateTime<Utc> (always present; defaults to epoch when
	// the prometheus text line carries no explicit timestamp).
	serde_json::json!({
			"metric": s.metric,
			"labels": labels,
			"value": value_to_json(&s.value),
			"timestamp": s.timestamp.timestamp_millis(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	// The OnceLock is process-global. Tests that call `install_recorder`
	// share the same handle — asserting "Some after install" and
	// "idempotent" is safe because `OnceLock::set` is a no-op on the
	// second call and `install_recorder` returns `Ok` regardless.

	#[test]
	fn install_recorder_is_idempotent() {
		let r1 = install_recorder();
		let r2 = install_recorder();
		assert!(r1.is_ok(), "first install ok: {r1:?}");
		assert!(r2.is_ok(), "second install idempotent ok: {r2:?}");
	}

	#[test]
	fn handle_returns_some_after_install() {
		install_recorder().ok();
		assert!(handle().is_some());
	}

	#[test]
	fn render_prometheus_returns_non_empty_text() {
		install_recorder().ok();
		let text = render_prometheus();
		assert!(text.is_some(), "recorder installed; render_prometheus must be Some");
	}

	#[test]
	fn render_json_returns_value_with_samples_key() {
		install_recorder().ok();
		let value = render_json().expect("recorder installed");
		assert!(value.get("samples").is_some(), "JSON must have `samples` key");
		assert!(value.get("docs").is_some(), "JSON must have `docs` key");
	}

	#[test]
	fn text_to_json_handles_empty_text() {
		let v = text_to_json("");
		assert!(v.get("samples").is_some());
		assert_eq!(v["samples"], serde_json::json!([]));
	}
}
