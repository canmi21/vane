#![allow(clippy::module_name_repetitions)]

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

	#[must_use]
	pub fn internal(msg: impl Into<Cow<'static, str>>) -> Self {
		Self::new(ErrorKind::Internal).with_ctx(msg)
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
			ErrorKind::Timeout(TimeoutKind::Connect)
			| ErrorKind::Resource(ResourceKind::ConnectionPool) => true,
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

// `Elapsed` carries no discriminator for which timeout tripped, so the
// conversion picks the most general bucket; sites that know the specific
// phase should build the error explicitly via `Error::timeout(kind)`.
impl From<tokio::time::error::Elapsed> for Error {
	fn from(e: tokio::time::error::Elapsed) -> Self {
		from_source(ErrorKind::Timeout(TimeoutKind::Total), e)
	}
}

// TODO: S2-XX — add `From<hyper::Error>` when hyper lands in engine deps.
// TODO: S2-XX — add `From<h3::Error>` when h3 lands in engine deps.
// TODO: S2-XX — add `From<rustls::Error>` when rustls lands in engine deps.
// TODO: S2-XX — add `From<hickory_resolver::ResolveError>` when hickory lands in engine deps.

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
