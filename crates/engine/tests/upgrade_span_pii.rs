//! Regression guard for the URL-path PII leak that previously
//! lived in `engine/src/upgrade.rs`'s `tracing::info_span!("request",
//! ...)` definitions (H1 / H2 / H3 drivers — three sites).
//!
//! URL paths routinely carry tokens (verify / reset / OAuth state),
//! user / tenant / order IDs, and other PII. The H1 / H2 / H3 driver
//! spans run at INFO; `tracing_subscriber::fmt::Layer` (the daemon's
//! stderr / journald log) interpolates a span's fields into every
//! event emitted inside that span, so a `path = %vane_req.uri().path()`
//! field would surface to anyone reading the system log. The fix is
//! to never put `path` (or any other PII-prone full-URI rendering)
//! on a request span; debugging by path goes through the flow log,
//! which is per-rule opt-in and explicit about what it captures.
//!
//! This test is structural rather than behavioural — it greps the
//! `upgrade.rs` source for the forbidden patterns. A behavioural
//! test using `tracing_subscriber::fmt::Layer` capturing into a
//! buffer + driving a synthetic H1 request would catch the same
//! regression, but at the cost of carrying a global subscriber state
//! through the test suite. The structural check has no such
//! coupling, and the forbidden literal is unmistakable.

const UPGRADE_RS: &str = include_str!("../src/upgrade.rs");

#[test]
fn no_uri_path_field_in_upgrade_info_spans() {
	// Match any line that puts the request's URI path on a tracing
	// span / event. The literal `uri().path()` token is the unique
	// signature; both `path = %...uri().path()` and any future
	// near-miss like `requested_path = %req.uri().path()` will trip
	// it.
	for (i, line) in UPGRADE_RS.lines().enumerate() {
		assert!(
			!line.contains("uri().path()"),
			"crates/engine/src/upgrade.rs line {} reintroduces a full-URI-path span field: {:?}. \
			 Paths commonly carry tokens / user IDs / tenant IDs; routing them through a \
			 tracing span surfaces them to anyone reading the daemon's structured log. Use the \
			 flow log (per-rule opt-in) for path-level debugging instead.",
			i + 1,
			line.trim(),
		);
	}
}

#[test]
fn upgrade_rs_keeps_method_field_on_request_spans() {
	// Sanity: the dimmed spans still carry `method`, which is safe
	// (HTTP method enum is fixed-cardinality) and is what makes the
	// log usable for coarse routing. If a refactor accidentally
	// strips the entire span, this catches it.
	assert!(
		UPGRADE_RS.contains("method = %vane_req.method()"),
		"upgrade.rs request spans must keep `method = %vane_req.method()` — \
		 stripping it leaves spans without any routing signal at all",
	);
}
