use std::borrow::Cow;

pub const SERIALIZED_MESSAGE_CAP: usize = 4 * 1024;
pub const SERIALIZED_CTX_CAP: usize = 1024;
pub const SERIALIZED_CHAIN_MAX_ENTRIES: usize = 16;
pub const SERIALIZED_CHAIN_ENTRY_CAP: usize = 1024;

#[derive(thiserror::Error, Debug)]
#[error("{kind}{}", .ctx.as_deref().map(|c| format!(": {c}")).unwrap_or_default())]
pub struct Error {
	pub kind: ErrorKind,
	pub ctx: Option<Cow<'static, str>>,
	#[source]
	pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
	#[error("i/o")]
	Io,
	#[error("protocol")]
	Protocol,
	#[error("upstream: {0}")]
	Upstream(UpstreamReason),
	#[error("middleware")]
	Middleware,
	#[error("compile")]
	Compile,
	#[error("timeout: {0}")]
	Timeout(TimeoutKind),
	#[error("canceled")]
	Canceled,
	#[error("resource: {0}")]
	Resource(ResourceKind),
	#[error("internal")]
	Internal,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum UpstreamReason {
	#[error("unreachable")]
	Unreachable,
	#[error("reset mid-request")]
	ResetMidRequest,
	#[error("reset on idle pickup")]
	ResetOnIdlePickup,
	#[error("tls handshake failed")]
	TlsHandshake,
	#[error("dns resolution failed")]
	DnsFailure,
	#[error("refused by upstream")]
	Refused,
	#[error("gone")]
	Gone,
	#[error("malformed response")]
	Malformed,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum TimeoutKind {
	#[error("connect")]
	Connect,
	#[error("read")]
	Read,
	#[error("total")]
	Total,
	#[error("idle")]
	Idle,
	#[error("handshake")]
	Handshake,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum ResourceKind {
	#[error("connection pool exhausted")]
	ConnectionPool,
	#[error("wasm pool exhausted")]
	WasmPool,
	#[error("memory budget exceeded")]
	Memory,
	#[error("file descriptors exhausted")]
	FdExhausted,
}

impl Error {
	#[must_use]
	pub const fn new(kind: ErrorKind) -> Self {
		Self { kind, ctx: None, source: None }
	}

	#[must_use]
	pub fn with_ctx(mut self, ctx: impl Into<Cow<'static, str>>) -> Self {
		self.ctx = Some(ctx.into());
		self
	}

	#[must_use]
	pub fn with_source<E: Into<Box<dyn std::error::Error + Send + Sync>>>(mut self, e: E) -> Self {
		self.source = Some(e.into());
		self
	}

	#[must_use]
	pub fn io(msg: impl Into<Cow<'static, str>>) -> Self {
		Self::new(ErrorKind::Io).with_ctx(msg)
	}

	#[must_use]
	pub fn protocol(msg: impl Into<Cow<'static, str>>) -> Self {
		Self::new(ErrorKind::Protocol).with_ctx(msg)
	}

	#[must_use]
	pub const fn upstream(reason: UpstreamReason) -> Self {
		Self::new(ErrorKind::Upstream(reason))
	}

	#[must_use]
	pub fn middleware(msg: impl Into<Cow<'static, str>>) -> Self {
		Self::new(ErrorKind::Middleware).with_ctx(msg)
	}

	#[must_use]
	pub fn compile(msg: impl Into<Cow<'static, str>>) -> Self {
		Self::new(ErrorKind::Compile).with_ctx(msg)
	}

	#[must_use]
	pub const fn timeout(kind: TimeoutKind) -> Self {
		Self::new(ErrorKind::Timeout(kind))
	}

	#[must_use]
	pub const fn canceled() -> Self {
		Self::new(ErrorKind::Canceled)
	}

	#[must_use]
	pub const fn resource(kind: ResourceKind) -> Self {
		Self::new(ErrorKind::Resource(kind))
	}

	/// Build an `ErrorKind::Internal` carrier for a detected invariant
	/// violation.
	///
	/// **Reserved for invariant breaks.** The error class signals that
	/// the code has reached a state the type system or the lower-pass
	/// invariants were supposed to make unreachable — examples in this
	/// codebase are `l4_forward` receiving an unexpected `L4Conn`
	/// variant, the executor finding the dispatch table missing from
	/// `ConnContext.user`, or a response builder rejecting bytes we
	/// validated upstream. Runtime user-data failures
	/// (`std::io::Error`, WASM trap, hyper-build mismatch on operator-
	/// controlled bytes) belong on `Error::middleware` / `Error::io` /
	/// `Error::protocol` instead.
	///
	/// In debug / test builds the constructor `debug_assert!`s false so
	/// the panic surfaces locally with the message context — invariant
	/// breaks are bugs that deserve to be found at dev time, not
	/// silently 500ed in production. Release builds keep the cheap
	/// `Error` construction path.
	#[must_use]
	#[track_caller]
	pub fn internal(msg: impl Into<Cow<'static, str>>) -> Self {
		let ctx = msg.into();
		// Allow tests to construct `Error::internal(...)` as a fixture
		// (e.g. asserting downstream code surfaces it correctly).
		// Non-test debug builds panic so dev iterations catch the
		// invariant break immediately.
		#[cfg(all(debug_assertions, not(test)))]
		debug_assert!(false, "Error::internal invariant violation: {ctx}");
		Self::new(ErrorKind::Internal).with_ctx(ctx)
	}

	#[must_use]
	pub const fn kind(&self) -> &ErrorKind {
		&self.kind
	}

	#[must_use]
	pub fn ctx(&self) -> Option<&str> {
		self.ctx.as_deref()
	}

	#[must_use]
	pub const fn kind_label(&self) -> &'static str {
		match &self.kind {
			ErrorKind::Io => "io",
			ErrorKind::Protocol => "protocol",
			ErrorKind::Upstream(_) => "upstream",
			ErrorKind::Middleware => "middleware",
			ErrorKind::Compile => "compile",
			ErrorKind::Timeout(_) => "timeout",
			ErrorKind::Canceled => "canceled",
			ErrorKind::Resource(_) => "resource",
			ErrorKind::Internal => "internal",
		}
	}

	#[must_use]
	pub const fn reason_label(&self) -> Option<&'static str> {
		match &self.kind {
			ErrorKind::Upstream(r) => Some(match r {
				UpstreamReason::Unreachable => "unreachable",
				UpstreamReason::ResetMidRequest => "reset_mid_request",
				UpstreamReason::ResetOnIdlePickup => "reset_idle_pickup",
				UpstreamReason::TlsHandshake => "tls_handshake",
				UpstreamReason::DnsFailure => "dns_failure",
				UpstreamReason::Refused => "refused",
				UpstreamReason::Gone => "gone",
				UpstreamReason::Malformed => "malformed",
			}),
			ErrorKind::Timeout(t) => Some(match t {
				TimeoutKind::Connect => "connect",
				TimeoutKind::Read => "read",
				TimeoutKind::Total => "total",
				TimeoutKind::Idle => "idle",
				TimeoutKind::Handshake => "handshake",
			}),
			ErrorKind::Resource(r) => Some(match r {
				ResourceKind::ConnectionPool => "connection_pool",
				ResourceKind::WasmPool => "wasm_pool",
				ResourceKind::Memory => "memory",
				ResourceKind::FdExhausted => "fd_exhausted",
			}),
			_ => None,
		}
	}

	/// Method-agnostic retry eligibility. Returns true for the
	/// pre-connect failures (request never left the wire), plus the
	/// hyper-pool race cases (`ResetOnIdlePickup`, `Refused`,
	/// `Gone`) and DNS / unreachable / connect-timeout — all of
	/// which are safe to retry regardless of HTTP method idempotency.
	///
	/// Mid-request failures (`ResetMidRequest`) need a method check
	/// before retrying so we don't double-deliver a POST body. Use
	/// [`Self::is_retryable_in`] for that path; this method exists
	/// for back-compat with callers that already pre-gate on method
	/// idempotency.
	#[must_use]
	pub const fn is_retryable(&self) -> bool {
		match &self.kind {
			ErrorKind::Upstream(r) => matches!(
				r,
				UpstreamReason::Unreachable
					| UpstreamReason::ResetOnIdlePickup
					| UpstreamReason::DnsFailure
					| UpstreamReason::Refused
					| UpstreamReason::Gone
			),
			ErrorKind::Timeout(TimeoutKind::Connect | TimeoutKind::Handshake)
			| ErrorKind::Resource(ResourceKind::ConnectionPool) => true,
			_ => false,
		}
	}

	/// Method-aware retry eligibility, per `spec/crates/engine.md`
	/// § _Error classification_:
	///
	/// - Pre-connect failures (TCP connect, TLS handshake, DNS,
	///   connection-pool exhaustion, hyper-pool idle-pickup race)
	///   return `true` regardless of method — the request never left
	///   the wire, so retrying a POST is safe.
	/// - Mid-request failures (`ResetMidRequest`) return `true`
	///   ONLY for idempotent methods (GET / HEAD / PUT / DELETE /
	///   OPTIONS, per RFC 9110 § 9.2.2). Retrying a non-idempotent
	///   POST mid-request risks double-delivery.
	/// - All other error kinds return `false`.
	///
	/// `Method::TRACE` is treated as non-idempotent in this table
	/// — RFC 9110 lists it as idempotent but middleboxes routinely
	/// rewrite it, so retrying is rarely the right move at proxy
	/// scope.
	#[must_use]
	pub fn is_retryable_in(&self, method: &http::Method) -> bool {
		use http::Method;
		match &self.kind {
			// Pre-connect failures: request body never reached the
			// upstream wire, retry is always safe.
			ErrorKind::Timeout(TimeoutKind::Connect | TimeoutKind::Handshake)
			| ErrorKind::Resource(ResourceKind::ConnectionPool)
			| ErrorKind::Upstream(
				UpstreamReason::TlsHandshake
				| UpstreamReason::DnsFailure
				| UpstreamReason::Unreachable
				| UpstreamReason::Refused
				| UpstreamReason::ResetOnIdlePickup,
			) => true,
			// Mid-request failures: only idempotent methods retry.
			ErrorKind::Upstream(UpstreamReason::ResetMidRequest | UpstreamReason::Gone) => matches!(
				*method,
				Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS
			),
			_ => false,
		}
	}

	#[must_use]
	pub const fn http_status(&self) -> u16 {
		match &self.kind {
			ErrorKind::Protocol => 400,
			ErrorKind::Upstream(_) => 502,
			ErrorKind::Timeout(_) => 504,
			ErrorKind::Resource(_) => 503,
			ErrorKind::Canceled => 499,
			ErrorKind::Middleware | ErrorKind::Compile | ErrorKind::Internal | ErrorKind::Io => 500,
		}
	}

	#[must_use]
	pub fn source_chain(&self) -> Vec<String> {
		let mut out = Vec::new();
		let mut cur: &dyn std::error::Error = self;
		while let Some(src) = cur.source() {
			out.push(src.to_string());
			cur = src;
		}
		out
	}

	/// Display adapter that renders the error in a richer one-line
	/// form suitable for `tracing::error!(error = %e.tracing(), …)`.
	///
	/// Layout:
	/// ```text
	/// <Display> reason=<reason?> chain=[<src> / <src> / …]
	/// ```
	///
	/// Drop-in replacement for `error = %e`. `kind` is already
	/// embedded in the `Display` impl (`<kind>{ctx}`); `reason` and
	/// `chain` add the structured fields that operator post-mortems
	/// otherwise lose. Released by `to_string()`; no extra allocations
	/// at construction time.
	#[must_use]
	pub fn tracing(&self) -> ErrorTracing<'_> {
		ErrorTracing(self)
	}
}

/// Display adapter — see [`Error::tracing`].
pub struct ErrorTracing<'a>(&'a Error);

impl std::fmt::Display for ErrorTracing<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.0)?;
		if let Some(reason) = self.0.reason_label() {
			write!(f, " reason={reason}")?;
		}
		let chain = self.0.source_chain();
		if !chain.is_empty() {
			write!(f, " chain=[{}]", chain.join(" / "))?;
		}
		Ok(())
	}
}

/// Accumulator for compile-pipeline diagnostics.
///
/// Each stage in `merge → expand → analyze → lower → validate` runs
/// per-rule / per-node "leaf checks" against the input. Historically
/// every leaf check used `?` to early-return, which meant an operator
/// running `vane compile <dir>` only ever saw the first error: fix
/// that, re-run, see the next, fix that, re-run, etc. With
/// `Diagnostics`, leaf checks `push` instead of `?`-returning and the
/// stage boundary decides whether to bail with the full accumulator
/// or continue into the next stage.
///
/// Every entry currently has the same severity (compile error). The
/// `has_fatal` helper exists so callers can express the "any error
/// stops the next stage" gate clearly at stage boundaries; future
/// warning-level diagnostics would slot in without changing call
/// sites.
#[derive(Debug, Default)]
pub struct Diagnostics {
	entries: Vec<Error>,
}

impl Diagnostics {
	#[must_use]
	pub const fn new() -> Self {
		Self { entries: Vec::new() }
	}

	pub fn push(&mut self, e: Error) {
		self.entries.push(e);
	}

	pub fn extend<I: IntoIterator<Item = Error>>(&mut self, iter: I) {
		self.entries.extend(iter);
	}

	#[must_use]
	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	#[must_use]
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	/// True when the accumulator carries at least one error that
	/// should stop the pipeline at the next stage boundary. Equivalent
	/// to `!is_empty()` today; reserved as a hook for warning-level
	/// diagnostics that might land here in the future.
	#[must_use]
	pub fn has_fatal(&self) -> bool {
		!self.entries.is_empty()
	}

	#[must_use]
	pub fn entries(&self) -> &[Error] {
		&self.entries
	}

	#[must_use]
	pub fn into_errors(self) -> Vec<Error> {
		self.entries
	}

	/// Stage-boundary gate. Returns `Ok(value)` when no diagnostics
	/// have been pushed; otherwise returns `Err(Self)` so the caller
	/// can either bubble or merge it into another accumulator.
	///
	/// # Errors
	/// Returns `self` when `has_fatal()` is true.
	pub fn into_result<T>(self, value: T) -> Result<T, Self> {
		if self.has_fatal() { Err(self) } else { Ok(value) }
	}
}

impl std::fmt::Display for Diagnostics {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self.entries.len() {
			0 => write!(f, "no diagnostics"),
			1 => write!(f, "{}", self.entries[0]),
			n => {
				writeln!(f, "{n} compile errors:")?;
				for (i, e) in self.entries.iter().enumerate() {
					writeln!(f, "  [{}/{n}] {e}", i + 1)?;
				}
				Ok(())
			}
		}
	}
}

impl From<Error> for Diagnostics {
	fn from(e: Error) -> Self {
		Self { entries: vec![e] }
	}
}

/// Collapse the accumulated diagnostics into a single
/// [`ErrorKind::Compile`] `Error` whose context carries every entry's
/// `to_string()`, separated by `\n`. Used at the boundary into APIs
/// whose error channel is a single `Error` (e.g. the existing
/// `compile()` facade, the management-RPC wire payload).
impl From<Diagnostics> for Error {
	fn from(d: Diagnostics) -> Self {
		match d.entries.len() {
			0 => Error::compile("no diagnostics"),
			1 => d.entries.into_iter().next().expect("len == 1"),
			n => {
				use std::fmt::Write as _;
				let mut joined = format!("{n} compile errors:");
				for (i, e) in d.entries.iter().enumerate() {
					let _ = write!(joined, "\n  [{}/{n}] {e}", i + 1);
				}
				Error::compile(joined)
			}
		}
	}
}

fn from_source<E>(kind: ErrorKind, e: E) -> Error
where
	E: std::error::Error + Send + Sync + 'static,
{
	Error { kind, ctx: None, source: Some(Box::new(e)) }
}

impl From<std::io::Error> for Error {
	fn from(e: std::io::Error) -> Self {
		from_source(ErrorKind::Io, e)
	}
}

impl From<serde_json::Error> for Error {
	fn from(e: serde_json::Error) -> Self {
		from_source(ErrorKind::Compile, e)
	}
}

impl From<fancy_regex::Error> for Error {
	fn from(e: fancy_regex::Error) -> Self {
		from_source(ErrorKind::Compile, e)
	}
}

impl From<ipnet::AddrParseError> for Error {
	fn from(e: ipnet::AddrParseError) -> Self {
		from_source(ErrorKind::Compile, e)
	}
}

// `From<tokio::time::error::Elapsed>` is intentionally not provided.
// `Elapsed` carries no discriminator for which timeout tripped, and a
// blanket conversion swept every timeout site into `TimeoutKind::Total`
// regardless of the actual stage (connect, read, header, etc.) —
// observers and retry classifiers then lost the distinction. Use
// [`timeout_with`] at the call site instead so the stage is named
// explicitly.

/// Run `fut` under a tokio timeout and translate the elapsed case into
/// a named [`TimeoutKind`]. Replaces the previous `From<Elapsed>` impl
/// so every call site spells out which stage owns the timeout.
///
/// # Errors
/// On expiry returns [`Error::timeout`]; otherwise propagates
/// `fut`'s own `Result`.
pub async fn timeout_with<T, E, F>(
	kind: TimeoutKind,
	duration: std::time::Duration,
	fut: F,
) -> Result<T, Error>
where
	F: std::future::Future<Output = Result<T, E>>,
	Error: From<E>,
{
	match tokio::time::timeout(duration, fut).await {
		Ok(Ok(v)) => Ok(v),
		Ok(Err(e)) => Err(Error::from(e)),
		Err(_) => Err(Error::timeout(kind)),
	}
}

// `From<hyper::Error>` / `h3::Error` / `rustls::Error` /
// `hickory_resolver::ResolveError` deliberately not implemented here:
// vane-core is backend-agnostic, and adding those impls (orphan rules
// require they live next to the local type) would force every transport
// crate into core's dep graph. Engine code constructs upstream errors
// explicitly via `Error::upstream(...).with_source(e)` so the
// `ErrorKind` / `UpstreamReason` is chosen at the call site rather
// than baked into a blanket `From` impl.

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SerializedError {
	pub kind: String,
	pub reason: Option<String>,
	pub message: String,
	pub ctx: Option<String>,
	pub source_chain: Vec<String>,
	pub http_status: u16,
	pub retryable: bool,
}

impl From<&Error> for SerializedError {
	fn from(e: &Error) -> Self {
		Self {
			kind: e.kind_label().to_owned(),
			reason: e.reason_label().map(ToOwned::to_owned),
			message: cap_bytes(e.to_string(), SERIALIZED_MESSAGE_CAP),
			ctx: e.ctx.as_deref().map(|c| cap_bytes(c.to_owned(), SERIALIZED_CTX_CAP)),
			source_chain: cap_chain(e.source_chain()),
			http_status: e.http_status(),
			retryable: e.is_retryable(),
		}
	}
}

const TRUNC_SUFFIX: &str = "… [truncated]";

fn cap_bytes(s: String, cap: usize) -> String {
	if s.len() <= cap {
		return s;
	}
	let budget = cap.saturating_sub(TRUNC_SUFFIX.len());
	let mut end = budget.min(s.len());
	while end > 0 && !s.is_char_boundary(end) {
		end -= 1;
	}
	let mut out = String::with_capacity(end + TRUNC_SUFFIX.len());
	out.push_str(&s[..end]);
	out.push_str(TRUNC_SUFFIX);
	out
}

fn cap_chain(chain: Vec<String>) -> Vec<String> {
	if chain.len() <= SERIALIZED_CHAIN_MAX_ENTRIES {
		return chain.into_iter().map(|s| cap_bytes(s, SERIALIZED_CHAIN_ENTRY_CAP)).collect();
	}
	let keep = SERIALIZED_CHAIN_MAX_ENTRIES - 1;
	let dropped = chain.len() - keep;
	let mut out: Vec<String> =
		chain.into_iter().take(keep).map(|s| cap_bytes(s, SERIALIZED_CHAIN_ENTRY_CAP)).collect();
	out.push(format!("… [{dropped} more]"));
	out
}

#[cfg(test)]
mod diagnostics_tests {
	use super::{Diagnostics, Error};

	#[test]
	fn empty_diagnostics_into_result_returns_ok_value() {
		let d = Diagnostics::new();
		assert!(d.is_empty());
		assert!(!d.has_fatal());
		let r: Result<u32, Diagnostics> = d.into_result(42);
		assert_eq!(r.unwrap(), 42);
	}

	#[test]
	fn non_empty_diagnostics_into_result_surfaces_self() {
		let mut d = Diagnostics::new();
		d.push(Error::compile("first"));
		d.push(Error::compile("second"));
		assert_eq!(d.len(), 2);
		assert!(d.has_fatal());
		let r: Result<(), Diagnostics> = d.into_result(());
		let got = r.expect_err("non-empty must be Err");
		assert_eq!(got.len(), 2);
	}

	#[test]
	fn diagnostics_display_lists_every_entry_with_numbered_prefix() {
		let mut d = Diagnostics::new();
		d.push(Error::compile("alpha"));
		d.push(Error::compile("beta"));
		let s = d.to_string();
		assert!(s.contains("2 compile errors"), "{s}");
		assert!(s.contains("[1/2]") && s.contains("alpha"), "{s}");
		assert!(s.contains("[2/2]") && s.contains("beta"), "{s}");
	}

	#[test]
	fn diagnostics_to_single_error_joins_messages_under_compile_kind() {
		let mut d = Diagnostics::new();
		d.push(Error::compile("alpha"));
		d.push(Error::compile("beta"));
		let collapsed: Error = d.into();
		let msg = collapsed.to_string();
		assert!(msg.contains("alpha"));
		assert!(msg.contains("beta"));
		assert!(matches!(collapsed.kind, super::ErrorKind::Compile));
	}

	#[test]
	fn single_error_diagnostics_collapses_to_that_error_verbatim() {
		let mut d = Diagnostics::new();
		d.push(Error::compile("solo"));
		let collapsed: Error = d.into();
		assert_eq!(collapsed.to_string(), Error::compile("solo").to_string());
	}
}
