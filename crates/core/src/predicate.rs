use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::Arc;

use bytes::Bytes;
use ipnet::IpNet;

use crate::body::Request;
use crate::conn_context::ConnContext;

#[derive(Clone, Eq, PartialEq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldPath {
	Transport,
	RemoteIp,
	RemotePort,
	LocalIp,
	LocalPort,
	Peek,
	TlsSni,
	TlsAlpn,
	TlsVersion,
	/// Was a verified peer cert presented this connection? Reads
	/// `tls.peer_cert.is_some()`. Bool-typed.
	TlsPeerCertPresent,
	TlsPeerCertSubjectCn,
	/// DNS-type Subject Alternative Names from the verified peer
	/// cert. `Vec<Str>`-typed: `contains`/`not_contains` against a
	/// single-element operand are the only legal operators (per
	/// `spec/crates/core.md` § _Operator × value
	/// type compatibility_).
	TlsPeerCertSanDns,
	/// SHA-256 of the full DER-encoded leaf cert, lowercase hex.
	TlsPeerCertFingerprintSha256,
	/// SHA-256 of the cert's `SubjectPublicKeyInfo` (rotation-stable
	/// pin), lowercase hex.
	TlsPeerCertSpkiSha256,
	/// Issuer Common Name — useful for routing on which internal CA
	/// signed the client cert.
	TlsPeerCertIssuerCn,
	/// Cert serial number, lowercase hex, big-endian, no
	/// leading-zero stripping.
	TlsPeerCertSerial,
	HttpMethod,
	HttpUriPath,
	HttpUriQuery,
	HttpHeader(Arc<str>),
	HttpBody,
}

/// Value type a [`FieldPath`] reads from. Drives the operator
/// compatibility matrix in `spec/crates/core.md`
/// § _Operator × value type compatibility_ and the `coerce_value`
/// validator in the lower pass.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum FieldValueType {
	Str,
	Bytes,
	Int,
	IpAddr,
	Enum,
	Bool,
	VecStr,
}

impl FieldValueType {
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Str => "Str",
			Self::Bytes => "Bytes",
			Self::Int => "Int",
			Self::IpAddr => "IpAddr",
			Self::Enum => "enum",
			Self::Bool => "Bool",
			Self::VecStr => "Vec<Str>",
		}
	}
}

impl FieldPath {
	/// Authoritative `FieldPath` → value type mapping. Mirrors the
	/// "Authoritative field paths" table in
	/// `spec/crates/core.md`.
	#[must_use]
	pub fn value_type(&self) -> FieldValueType {
		match self {
			Self::Transport | Self::TlsVersion | Self::HttpMethod => FieldValueType::Enum,
			Self::RemoteIp | Self::LocalIp => FieldValueType::IpAddr,
			Self::RemotePort | Self::LocalPort => FieldValueType::Int,
			Self::Peek | Self::TlsAlpn | Self::HttpBody => FieldValueType::Bytes,
			Self::TlsPeerCertPresent => FieldValueType::Bool,
			Self::TlsPeerCertSanDns => FieldValueType::VecStr,
			Self::TlsSni
			| Self::TlsPeerCertSubjectCn
			| Self::TlsPeerCertFingerprintSha256
			| Self::TlsPeerCertSpkiSha256
			| Self::TlsPeerCertIssuerCn
			| Self::TlsPeerCertSerial
			| Self::HttpUriPath
			| Self::HttpUriQuery
			| Self::HttpHeader(_) => FieldValueType::Str,
		}
	}

	/// Stable display label for diagnostic messages.
	#[must_use]
	pub fn display_name(&self) -> String {
		match self {
			Self::Transport => "transport".to_string(),
			Self::RemoteIp => "remote.ip".to_string(),
			Self::RemotePort => "remote.port".to_string(),
			Self::LocalIp => "local.ip".to_string(),
			Self::LocalPort => "local.port".to_string(),
			Self::Peek => "peek".to_string(),
			Self::TlsSni => "tls.sni".to_string(),
			Self::TlsAlpn => "tls.alpn".to_string(),
			Self::TlsVersion => "tls.version".to_string(),
			Self::TlsPeerCertPresent => "tls.peer_cert.present".to_string(),
			Self::TlsPeerCertSubjectCn => "tls.peer_cert.subject_cn".to_string(),
			Self::TlsPeerCertSanDns => "tls.peer_cert.san_dns".to_string(),
			Self::TlsPeerCertFingerprintSha256 => "tls.peer_cert.fingerprint_sha256".to_string(),
			Self::TlsPeerCertSpkiSha256 => "tls.peer_cert.spki_sha256".to_string(),
			Self::TlsPeerCertIssuerCn => "tls.peer_cert.issuer_cn".to_string(),
			Self::TlsPeerCertSerial => "tls.peer_cert.serial".to_string(),
			Self::HttpMethod => "http.method".to_string(),
			Self::HttpUriPath => "http.uri.path".to_string(),
			Self::HttpUriQuery => "http.uri.query".to_string(),
			Self::HttpHeader(name) => format!("http.header.{name}"),
			Self::HttpBody => "http.body".to_string(),
		}
	}
}

/// Operator family used by the type-compatibility matrix. Mirrors the
/// rows of `spec/crates/core.md`'s "Operator ×
/// value type compatibility" table — operators in the same row share a
/// compatibility set.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum OperatorFamily {
	Equality,
	StringSubstr,
	StringPrefSuf,
	RegexMatches,
	InList,
	NumericCmp,
	CidrMatch,
}

impl Operator {
	#[must_use]
	pub fn family(&self) -> OperatorFamily {
		match self {
			Self::Equals(_) | Self::NotEquals(_) => OperatorFamily::Equality,
			Self::Contains(_) | Self::NotContains(_) => OperatorFamily::StringSubstr,
			Self::Prefix(_) | Self::Suffix(_) => OperatorFamily::StringPrefSuf,
			Self::Matches(_) => OperatorFamily::RegexMatches,
			Self::In(_) | Self::NotIn(_) => OperatorFamily::InList,
			Self::Gt(_) | Self::Gte(_) | Self::Lt(_) | Self::Lte(_) => OperatorFamily::NumericCmp,
			Self::Cidr(_) => OperatorFamily::CidrMatch,
		}
	}

	#[must_use]
	pub fn name(&self) -> &'static str {
		match self {
			Self::Equals(_) => "equals",
			Self::NotEquals(_) => "not_equals",
			Self::Contains(_) => "contains",
			Self::NotContains(_) => "not_contains",
			Self::Prefix(_) => "prefix",
			Self::Suffix(_) => "suffix",
			Self::Matches(_) => "matches",
			Self::In(_) => "in",
			Self::NotIn(_) => "not_in",
			Self::Gt(_) => "gt",
			Self::Gte(_) => "gte",
			Self::Lt(_) => "lt",
			Self::Lte(_) => "lte",
			Self::Cidr(_) => "cidr",
		}
	}
}

impl OperatorFamily {
	/// Compatibility check from `spec/crates/core.md`
	/// § _Operator × value type compatibility_. The matrix is small and
	/// closed; enumerated here rather than data-driven so a future spec
	/// change forces a recompile-sized review.
	#[must_use]
	pub fn accepts(self, vt: FieldValueType) -> bool {
		use FieldValueType as V;
		use OperatorFamily as F;
		matches!(
			(self, vt),
			// Equality on every value type *except* Vec<Str>: equals
			// against a list literal isn't expressible in the JSON
			// schema (no Vec<Str> JSON value type) and would be
			// semantically the same as element-wise contains anyway.
			(F::Equality, V::Str | V::Bytes | V::Int | V::IpAddr | V::Enum | V::Bool)
				// In-list — same set as Equality minus Bool (no JSON
				// boolean array literal).
				| (F::InList, V::Str | V::Bytes | V::Int | V::IpAddr | V::Enum)
				| (F::StringSubstr, V::Str | V::Bytes | V::VecStr)
				| (F::StringPrefSuf, V::Str | V::Bytes)
				| (F::RegexMatches, V::Str)
				| (F::NumericCmp, V::Int)
				| (F::CidrMatch, V::IpAddr),
		)
	}

	/// Short human label for diagnostic messages.
	#[must_use]
	pub fn family_expectation(self) -> &'static str {
		match self {
			Self::Equality => "any of Str/Bytes/Int/IpAddr/enum/Bool",
			Self::InList => "any of Str/Bytes/Int/IpAddr/enum",
			Self::StringSubstr => "Str, Bytes, or Vec<Str>",
			Self::StringPrefSuf => "Str or Bytes",
			Self::RegexMatches => "Str",
			Self::NumericCmp => "numeric",
			Self::CidrMatch => "IpAddr",
		}
	}
}

#[derive(Clone, Debug)]
pub enum CompiledValue {
	Str(Arc<str>),
	Bytes(Bytes),
	Int(i64),
	Bool(bool),
	Addr(IpAddr),
}

impl PartialEq for CompiledValue {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::Str(a), Self::Str(b)) => a.as_ref() == b.as_ref(),
			(Self::Bytes(a), Self::Bytes(b)) => a == b,
			(Self::Int(a), Self::Int(b)) => a == b,
			(Self::Bool(a), Self::Bool(b)) => a == b,
			(Self::Addr(a), Self::Addr(b)) => a == b,
			_ => false,
		}
	}
}

impl Eq for CompiledValue {}

impl Hash for CompiledValue {
	fn hash<H: Hasher>(&self, state: &mut H) {
		std::mem::discriminant(self).hash(state);
		match self {
			Self::Str(s) => s.as_ref().hash(state),
			Self::Bytes(b) => b.hash(state),
			Self::Int(i) => i.hash(state),
			Self::Bool(b) => b.hash(state),
			Self::Addr(a) => a.hash(state),
		}
	}
}

#[derive(Clone, Debug)]
pub enum CompiledOperator {
	Equals(CompiledValue),
	NotEquals(CompiledValue),
	Contains(Bytes),
	NotContains(Bytes),
	Prefix(Bytes),
	Suffix(Bytes),
	Matches(fancy_regex::Regex),
	In(Vec<CompiledValue>),
	NotIn(Vec<CompiledValue>),
	Gt(i64),
	Gte(i64),
	Lt(i64),
	Lte(i64),
	Cidr(IpNet),
}

impl PartialEq for CompiledOperator {
	fn eq(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::Equals(a), Self::Equals(b)) | (Self::NotEquals(a), Self::NotEquals(b)) => a == b,
			(Self::Contains(a), Self::Contains(b))
			| (Self::NotContains(a), Self::NotContains(b))
			| (Self::Prefix(a), Self::Prefix(b))
			| (Self::Suffix(a), Self::Suffix(b)) => a == b,
			(Self::Matches(a), Self::Matches(b)) => a.as_str() == b.as_str(),
			(Self::In(a), Self::In(b)) | (Self::NotIn(a), Self::NotIn(b)) => a == b,
			(Self::Gt(a), Self::Gt(b))
			| (Self::Gte(a), Self::Gte(b))
			| (Self::Lt(a), Self::Lt(b))
			| (Self::Lte(a), Self::Lte(b)) => a == b,
			(Self::Cidr(a), Self::Cidr(b)) => a == b,
			_ => false,
		}
	}
}

impl Eq for CompiledOperator {}

impl Hash for CompiledOperator {
	fn hash<H: Hasher>(&self, state: &mut H) {
		std::mem::discriminant(self).hash(state);
		match self {
			Self::Equals(v) | Self::NotEquals(v) => v.hash(state),
			Self::Contains(b) | Self::NotContains(b) | Self::Prefix(b) | Self::Suffix(b) => {
				b.hash(state);
			}
			Self::Matches(r) => r.as_str().hash(state),
			Self::In(v) | Self::NotIn(v) => v.hash(state),
			Self::Gt(i) | Self::Gte(i) | Self::Lt(i) | Self::Lte(i) => i.hash(state),
			Self::Cidr(n) => n.hash(state),
		}
	}
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PredicateInst {
	pub path: FieldPath,
	pub op: CompiledOperator,
}

pub enum PredicateView<'a> {
	L4 { conn: &'a Arc<ConnContext>, peek: Option<&'a [u8]> },
	L7Req { conn: &'a Arc<ConnContext>, req: &'a Request },
}

impl<'a> PredicateView<'a> {
	/// Build the phase-appropriate view the executor hands to
	/// `PredicateInst::test`. Picks `L7Req` when a `Request` is in scope
	/// (phase `L7Request`), otherwise falls back to `L4`.
	///
	/// `peek` carries the bytes the listener-side prelude buffered on
	/// the connection — see `spec/crates/engine.md` § _Protocol
	/// detection_. The executor extracts it from `ConnContext.user`
	/// (where the listener stashed a `PeekResult`) and forwards a
	/// borrow with a lifetime that outlives this view.
	#[must_use]
	pub fn build(
		conn: &'a Arc<ConnContext>,
		req: Option<&'a Request>,
		_l4: Option<&'a crate::l4::L4Conn>,
		peek: Option<&'a [u8]>,
	) -> Self {
		match req {
			Some(r) => Self::L7Req { conn, req: r },
			None => Self::L4 { conn, peek },
		}
	}

	fn conn(&self) -> &Arc<ConnContext> {
		match self {
			Self::L4 { conn, .. } | Self::L7Req { conn, .. } => conn,
		}
	}

	fn request(&self) -> Option<&Request> {
		match self {
			Self::L7Req { req, .. } => Some(req),
			Self::L4 { .. } => None,
		}
	}

	fn peek_buffer(&self) -> Option<&[u8]> {
		match self {
			Self::L4 { peek, .. } => *peek,
			Self::L7Req { .. } => None,
		}
	}
}

impl PredicateInst {
	/// Evaluate the predicate against a phase-typed view. Path-reader →
	/// operator-family dispatch; the matrix of legal `(path, op)` pairs
	/// is enforced at compile (see [`Operator::family`] and
	/// [`OperatorFamily::accepts`]). Illegal pairs cannot reach this
	/// function in any compiled `FlowGraph`; they would have failed
	/// `compile_operator`. Hand-built `PredicateInst`s in tests that
	/// supply an unreachable pair fall through to `false` — sound-by-
	/// default per the spec's "missing fields miss" contract.
	///
	/// Reads that need an absent piece of state (e.g. `tls.sni` on a
	/// cleartext connection, `http.header.upgrade` from an `L4` view)
	/// also miss rather than panic.
	/// # Panics
	/// On the `http.body` arm, the path-reader calls
	/// `Body::as_static().expect("lazy-buffer invariant")`. The compile
	/// pass marks the incoming edge of every `http.body` Check with
	/// `collect_body_before = Some(BodySide::Request)`, so by the time
	/// `test()` runs the executor has already collected the request
	/// body into `Body::Static`. A `Body::Stream` / `Body::Empty`
	/// reaching this arm therefore signals a `FlowGraph` compile bug and
	/// is surfaced as a clear panic instead of a silent miss. All other
	/// arms are panic-free; absent state misses sound-by-default.
	#[must_use]
	#[allow(clippy::too_many_lines)]
	pub fn test(&self, view: &PredicateView<'_>) -> bool {
		match &self.path {
			FieldPath::Transport => {
				let s = match view.conn().transport {
					crate::conn_context::Transport::Tcp => "tcp",
					crate::conn_context::Transport::Udp => "udp",
				};
				test_str(&self.op, s)
			}
			FieldPath::RemoteIp => test_addr(&self.op, view.conn().remote.ip()),
			FieldPath::RemotePort => test_int(&self.op, i64::from(view.conn().remote.port())),
			FieldPath::LocalIp => test_addr(&self.op, view.conn().local.ip()),
			FieldPath::LocalPort => test_int(&self.op, i64::from(view.conn().local.port())),
			FieldPath::Peek => view.peek_buffer().is_some_and(|b| test_bytes(&self.op, b)),
			FieldPath::TlsSni => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.sni.clone())
				.is_some_and(|got| test_str(&self.op, got.as_str())),
			FieldPath::TlsAlpn => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.alpn.clone())
				.is_some_and(|got| test_bytes(&self.op, got.as_slice())),
			FieldPath::TlsVersion => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.version)
				.is_some_and(|v| test_str(&self.op, tls_version_str(v))),
			// `tls.peer_cert.*` reads the verified client certificate
			// captured at TLS handshake completion. The engine's
			// post-handshake hook pre-extracts every predicate-readable
			// field into `PeerCertificate`, so these readers just look
			// up cached scalars — no per-Check re-parse. Sound-by-
			// default: missing cert / missing field all miss (except
			// `present`, which returns false explicitly when the cert
			// is absent — by design, so the Request-mode pattern of
			// "branch on present == false" works).
			FieldPath::TlsPeerCertPresent => {
				let present = view.conn().tls.lock().as_ref().is_some_and(|t| t.peer_cert.is_some());
				test_bool(&self.op, present)
			}
			FieldPath::TlsPeerCertSubjectCn => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.peer_cert.as_ref().and_then(|p| p.subject_cn.clone()))
				.is_some_and(|cn| test_str(&self.op, cn.as_str())),
			FieldPath::TlsPeerCertSanDns => {
				let dns_list: Vec<String> = view
					.conn()
					.tls
					.lock()
					.as_ref()
					.and_then(|t| t.peer_cert.as_ref().map(|p| p.san_dns.clone()))
					.unwrap_or_default();
				test_vec_str(&self.op, &dns_list)
			}
			FieldPath::TlsPeerCertFingerprintSha256 => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.peer_cert.as_ref().map(|p| p.fingerprint_sha256.clone()))
				.is_some_and(|s| test_str(&self.op, s.as_str())),
			FieldPath::TlsPeerCertSpkiSha256 => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.peer_cert.as_ref().map(|p| p.spki_sha256.clone()))
				.is_some_and(|s| test_str(&self.op, s.as_str())),
			FieldPath::TlsPeerCertIssuerCn => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.peer_cert.as_ref().and_then(|p| p.issuer_cn.clone()))
				.is_some_and(|s| test_str(&self.op, s.as_str())),
			FieldPath::TlsPeerCertSerial => view
				.conn()
				.tls
				.lock()
				.as_ref()
				.and_then(|t| t.peer_cert.as_ref().map(|p| p.serial.clone()))
				.is_some_and(|s| test_str(&self.op, s.as_str())),
			FieldPath::HttpMethod => {
				let Some(req) = view.request() else { return false };
				test_str(&self.op, req.method().as_str())
			}
			FieldPath::HttpUriPath => {
				let Some(req) = view.request() else { return false };
				test_str(&self.op, req.uri().path())
			}
			FieldPath::HttpUriQuery => {
				let Some(req) = view.request() else { return false };
				test_str(&self.op, req.uri().query().unwrap_or(""))
			}
			// Header lookup: name is already lowercased at compile via
			// `parse_field_path`, and `HeaderMap::get` folds case on
			// the read side (RFC 9110 § 5.1). Value comparison is
			// byte-exact (RFC 9110 § 5.5). Multi-value headers expose
			// the first value only — predicates wanting "any of the
			// values" compose with `any_of` per
			// spec/crates/core.md § _http.header.<name>_.
			FieldPath::HttpHeader(name) => {
				let Some(req) = view.request() else { return false };
				let Some(value) = req.headers().get(name.as_ref()) else { return false };
				let Ok(s) = value.to_str() else {
					// Header values are byte-strings; non-UTF-8 misses
					// (Str-typed predicates can't compare to non-UTF-8
					// without lossy coercion, and silent loss is worse
					// than a miss).
					return false;
				};
				test_str(&self.op, s)
			}
			// `http.body` reads the request body bytes. Per
			// spec/crates/core.md § _Runtime_, the analyze pass marks
			// the incoming edge of any `http.body` Check with
			// `collect_body_before = Some(BodySide::Request)`, so by the
			// time `test()` runs the executor has already collected
			// `Body::Stream` into `Body::Static`. The `.expect` therefore
			// reflects an invariant of the compiled FlowGraph, not a
			// caller-side assumption.
			FieldPath::HttpBody => {
				let Some(req) = view.request() else { return false };
				let bytes = req.body().as_static().expect("lazy-buffer invariant");
				test_bytes(&self.op, bytes.as_ref())
			}
		}
	}
}

fn tls_version_str(v: crate::conn_context::TlsVersion) -> &'static str {
	match v {
		crate::conn_context::TlsVersion::Tls12 => "1.2",
		crate::conn_context::TlsVersion::Tls13 => "1.3",
	}
}

// `peer_cert_subject_cn` was the per-Check x509-parser invocation used
// before mTLS landed. The engine now pre-extracts every
// `tls.peer_cert.*` field once at handshake completion (see
// `PeerCertificate::from_der`); the per-Check predicate readers just
// look up the cached scalar.

/// Bool-typed reader. Per `spec/crates/core.md`
/// § _Operator × value type compatibility_, only `equals` /
/// `not_equals` against a Bool literal are legal; everything else
/// matrix-rejects at compile and falls through to `false` here as a
/// sound default.
fn test_bool(op: &CompiledOperator, value: bool) -> bool {
	match op {
		CompiledOperator::Equals(CompiledValue::Bool(expected)) => value == *expected,
		CompiledOperator::NotEquals(CompiledValue::Bool(expected)) => value != *expected,
		_ => false,
	}
}

/// `Vec<Str>`-typed reader. Per spec, only `contains` /
/// `not_contains` against a single-string operand are legal; the
/// semantics is "the list contains / does not contain this exact
/// element", not byte-level substring. Other operators
/// matrix-reject at compile.
fn test_vec_str(op: &CompiledOperator, values: &[String]) -> bool {
	match op {
		CompiledOperator::Contains(needle) => values.iter().any(|v| v.as_bytes() == needle.as_ref()),
		CompiledOperator::NotContains(needle) => {
			!values.iter().any(|v| v.as_bytes() == needle.as_ref())
		}
		_ => false,
	}
}

/// String-typed reader. Handles `equals`/`not_equals`,
/// `contains`/`not_contains`, `prefix`/`suffix`, `matches`,
/// `in`/`not_in`. Numeric and CIDR operators are matrix-rejected at
/// compile and fall through to `false` here as a sound default.
fn test_str(op: &CompiledOperator, value: &str) -> bool {
	match op {
		CompiledOperator::Equals(CompiledValue::Str(expected)) => value == expected.as_ref(),
		CompiledOperator::NotEquals(CompiledValue::Str(expected)) => value != expected.as_ref(),
		CompiledOperator::Contains(b) => contains_bytes(value.as_bytes(), b),
		CompiledOperator::NotContains(b) => !contains_bytes(value.as_bytes(), b),
		CompiledOperator::Prefix(b) => value.as_bytes().starts_with(b.as_ref()),
		CompiledOperator::Suffix(b) => value.as_bytes().ends_with(b.as_ref()),
		CompiledOperator::Matches(re) => re.is_match(value).unwrap_or(false),
		CompiledOperator::In(values) => {
			values.iter().any(|v| matches!(v, CompiledValue::Str(s) if value == s.as_ref()))
		}
		CompiledOperator::NotIn(values) => {
			!values.iter().any(|v| matches!(v, CompiledValue::Str(s) if value == s.as_ref()))
		}
		_ => false,
	}
}

/// Bytes-typed reader. `matches` (regex) is matrix-rejected; numeric/CIDR too.
/// `equals`/`in` compare against `CompiledValue::Bytes`; lower's
/// `coerce_value` produces that variant for Bytes-typed paths.
fn test_bytes(op: &CompiledOperator, value: &[u8]) -> bool {
	match op {
		CompiledOperator::Equals(CompiledValue::Bytes(expected)) => value == expected.as_ref(),
		CompiledOperator::NotEquals(CompiledValue::Bytes(expected)) => value != expected.as_ref(),
		CompiledOperator::Contains(b) => contains_bytes(value, b),
		CompiledOperator::NotContains(b) => !contains_bytes(value, b),
		CompiledOperator::Prefix(b) => value.starts_with(b.as_ref()),
		CompiledOperator::Suffix(b) => value.ends_with(b.as_ref()),
		CompiledOperator::In(values) => {
			values.iter().any(|v| matches!(v, CompiledValue::Bytes(b) if value == b.as_ref()))
		}
		CompiledOperator::NotIn(values) => {
			!values.iter().any(|v| matches!(v, CompiledValue::Bytes(b) if value == b.as_ref()))
		}
		_ => false,
	}
}

/// Int-typed reader. Handles `equals`/`not_equals`, `gt`/`gte`/`lt`/`lte`,
/// `in`/`not_in`. The remaining ops are matrix-rejected.
fn test_int(op: &CompiledOperator, value: i64) -> bool {
	match op {
		CompiledOperator::Equals(CompiledValue::Int(expected)) => value == *expected,
		CompiledOperator::NotEquals(CompiledValue::Int(expected)) => value != *expected,
		CompiledOperator::Gt(n) => value > *n,
		CompiledOperator::Gte(n) => value >= *n,
		CompiledOperator::Lt(n) => value < *n,
		CompiledOperator::Lte(n) => value <= *n,
		CompiledOperator::In(values) => {
			values.iter().any(|v| matches!(v, CompiledValue::Int(i) if value == *i))
		}
		CompiledOperator::NotIn(values) => {
			!values.iter().any(|v| matches!(v, CompiledValue::Int(i) if value == *i))
		}
		_ => false,
	}
}

/// IpAddr-typed reader. `equals`/`not_equals`, `in`/`not_in`, `cidr`.
/// Cross-family `in` lists (e.g. v4+v6) match iff any element matches —
/// a single `cidr` is single-family per spec 18 § _CIDR specifics_.
fn test_addr(op: &CompiledOperator, value: std::net::IpAddr) -> bool {
	match op {
		CompiledOperator::Equals(CompiledValue::Addr(expected)) => value == *expected,
		CompiledOperator::NotEquals(CompiledValue::Addr(expected)) => value != *expected,
		CompiledOperator::Cidr(net) => net.contains(&value),
		CompiledOperator::In(values) => {
			values.iter().any(|v| matches!(v, CompiledValue::Addr(a) if value == *a))
		}
		CompiledOperator::NotIn(values) => {
			!values.iter().any(|v| matches!(v, CompiledValue::Addr(a) if value == *a))
		}
		_ => false,
	}
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
	if needle.is_empty() {
		return true;
	}
	if needle.len() > haystack.len() {
		return false;
	}
	haystack.windows(needle.len()).any(|w| w == needle)
}

pub const REGEX_PATTERN_MAX_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, serde::Serialize)]
pub enum Predicate {
	AnyOf(AnyOfP),
	AllOf(AllOfP),
	Not(NotP),
	Check(CheckMap),
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnyOfP {
	pub any_of: Vec<Predicate>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AllOfP {
	pub all_of: Vec<Predicate>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct NotP {
	pub not: Box<Predicate>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckMap {
	pub path: FieldPath,
	pub op: Operator,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
	Equals(Value),
	NotEquals(Value),
	Contains(Value),
	NotContains(Value),
	Prefix(Value),
	Suffix(Value),
	Matches(String),
	In(Vec<Value>),
	NotIn(Vec<Value>),
	Gt(i64),
	Gte(i64),
	Lt(i64),
	Lte(i64),
	Cidr(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum Value {
	Bool(bool),
	Int(i64),
	Str(String),
}

impl<'de> serde::Deserialize<'de> for Predicate {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		let v = serde_json::Value::deserialize(de)?;
		let serde_json::Value::Object(ref map) = v else {
			return Err(serde::de::Error::custom("predicate must be a JSON object"));
		};
		if map.len() == 1 {
			let (key, _) = map.iter().next().expect("len == 1");
			match key.as_str() {
				"any_of" => {
					return serde_json::from_value::<AnyOfP>(v)
						.map(Predicate::AnyOf)
						.map_err(serde::de::Error::custom);
				}
				"all_of" => {
					return serde_json::from_value::<AllOfP>(v)
						.map(Predicate::AllOf)
						.map_err(serde::de::Error::custom);
				}
				"not" => {
					return serde_json::from_value::<NotP>(v)
						.map(Predicate::Not)
						.map_err(serde::de::Error::custom);
				}
				_ => {}
			}
		}
		serde_json::from_value::<CheckMap>(v).map(Predicate::Check).map_err(serde::de::Error::custom)
	}
}

impl<'de> serde::Deserialize<'de> for CheckMap {
	fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
		struct Visitor;

		impl<'de> serde::de::Visitor<'de> for Visitor {
			type Value = CheckMap;

			fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("a single-key object of the form {\"<field-path>\": {\"<operator>\": <value>}}")
			}

			fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<CheckMap, M::Error> {
				let Some(key) = map.next_key::<String>()? else {
					return Err(serde::de::Error::invalid_length(0, &"exactly one key"));
				};
				let path = parse_field_path(&key).map_err(serde::de::Error::custom)?;
				let op: Operator = map.next_value()?;
				if map.next_key::<serde::de::IgnoredAny>()?.is_some() {
					return Err(serde::de::Error::custom("check object must have exactly one key"));
				}
				validate_operator(&op).map_err(serde::de::Error::custom)?;
				Ok(CheckMap { path, op })
			}
		}

		de.deserialize_map(Visitor)
	}
}

fn parse_field_path(s: &str) -> Result<FieldPath, String> {
	if s.chars().any(|c| c.is_ascii_uppercase()) {
		return Err(format!(
			"field path must be lowercase: {:?} — did you mean {:?}?",
			s,
			s.to_ascii_lowercase(),
		));
	}
	match s {
		"transport" => Ok(FieldPath::Transport),
		"remote.ip" => Ok(FieldPath::RemoteIp),
		"remote.port" => Ok(FieldPath::RemotePort),
		"local.ip" => Ok(FieldPath::LocalIp),
		"local.port" => Ok(FieldPath::LocalPort),
		"peek" => Ok(FieldPath::Peek),
		"tls.sni" => Ok(FieldPath::TlsSni),
		"tls.alpn" => Ok(FieldPath::TlsAlpn),
		"tls.version" => Ok(FieldPath::TlsVersion),
		"tls.peer_cert.present" => Ok(FieldPath::TlsPeerCertPresent),
		"tls.peer_cert.subject_cn" => Ok(FieldPath::TlsPeerCertSubjectCn),
		"tls.peer_cert.san_dns" => Ok(FieldPath::TlsPeerCertSanDns),
		"tls.peer_cert.fingerprint_sha256" => Ok(FieldPath::TlsPeerCertFingerprintSha256),
		"tls.peer_cert.spki_sha256" => Ok(FieldPath::TlsPeerCertSpkiSha256),
		"tls.peer_cert.issuer_cn" => Ok(FieldPath::TlsPeerCertIssuerCn),
		"tls.peer_cert.serial" => Ok(FieldPath::TlsPeerCertSerial),
		"http.method" => Ok(FieldPath::HttpMethod),
		"http.uri.path" => Ok(FieldPath::HttpUriPath),
		"http.uri.query" => Ok(FieldPath::HttpUriQuery),
		"http.body" => Ok(FieldPath::HttpBody),
		other if other.starts_with("http.header.") => {
			let name = &other["http.header.".len()..];
			if name.is_empty() {
				return Err(format!("http.header.* requires a header name: {other:?}"));
			}
			Ok(FieldPath::HttpHeader(Arc::from(name)))
		}
		other => Err(format!("unknown field path: {other:?}")),
	}
}

fn validate_operator(op: &Operator) -> Result<(), String> {
	if let Operator::Matches(pattern) = op
		&& pattern.len() > REGEX_PATTERN_MAX_BYTES
	{
		return Err(format!(
			"regex pattern source exceeds {REGEX_PATTERN_MAX_BYTES}-byte limit: got {} bytes",
			pattern.len(),
		));
	}
	Ok(())
}

mod serde_impls {
	use base64::Engine as _;
	use base64::engine::general_purpose::STANDARD as B64;
	use bytes::Bytes;
	use std::net::IpAddr;
	use std::sync::Arc;

	use super::{CompiledOperator, CompiledValue};

	pub(super) fn ser_bytes<S: serde::Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
		s.serialize_str(&B64.encode(b))
	}

	pub(super) fn de_bytes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
		use serde::Deserialize as _;
		let s = String::deserialize(d)?;
		B64.decode(s.as_bytes()).map(Bytes::from).map_err(serde::de::Error::custom)
	}

	pub(super) fn ser_regex<S: serde::Serializer>(
		r: &fancy_regex::Regex,
		s: S,
	) -> Result<S::Ok, S::Error> {
		s.serialize_str(r.as_str())
	}

	pub(super) fn de_regex<'de, D: serde::Deserializer<'de>>(
		d: D,
	) -> Result<fancy_regex::Regex, D::Error> {
		use serde::Deserialize as _;
		let s = String::deserialize(d)?;
		fancy_regex::Regex::new(&s)
			.map_err(|e| serde::de::Error::custom(format!("invalid regex {s:?}: {e}")))
	}

	// Shadow for CompiledValue — externally-tagged snake_case.
	#[derive(serde::Serialize, serde::Deserialize)]
	#[serde(rename_all = "snake_case")]
	pub(super) enum ValueShadow {
		Str(Arc<str>),
		#[serde(serialize_with = "ser_bytes", deserialize_with = "de_bytes")]
		Bytes(Bytes),
		Int(i64),
		Bool(bool),
		Addr(IpAddr),
	}

	impl From<&CompiledValue> for ValueShadow {
		fn from(v: &CompiledValue) -> Self {
			match v {
				CompiledValue::Str(s) => Self::Str(Arc::clone(s)),
				CompiledValue::Bytes(b) => Self::Bytes(b.clone()),
				CompiledValue::Int(i) => Self::Int(*i),
				CompiledValue::Bool(b) => Self::Bool(*b),
				CompiledValue::Addr(a) => Self::Addr(*a),
			}
		}
	}

	impl From<ValueShadow> for CompiledValue {
		fn from(v: ValueShadow) -> Self {
			match v {
				ValueShadow::Str(s) => Self::Str(s),
				ValueShadow::Bytes(b) => Self::Bytes(b),
				ValueShadow::Int(i) => Self::Int(i),
				ValueShadow::Bool(b) => Self::Bool(b),
				ValueShadow::Addr(a) => Self::Addr(a),
			}
		}
	}

	// Shadow for CompiledOperator — variant names mirror parse-form Operator
	// (snake_case), so round-tripping a dry-run JSON preserves reader intuition.
	#[derive(serde::Serialize, serde::Deserialize)]
	#[serde(rename_all = "snake_case")]
	pub(super) enum OperatorShadow {
		Equals(CompiledValue),
		NotEquals(CompiledValue),
		#[serde(serialize_with = "ser_bytes", deserialize_with = "de_bytes")]
		Contains(Bytes),
		#[serde(serialize_with = "ser_bytes", deserialize_with = "de_bytes")]
		NotContains(Bytes),
		#[serde(serialize_with = "ser_bytes", deserialize_with = "de_bytes")]
		Prefix(Bytes),
		#[serde(serialize_with = "ser_bytes", deserialize_with = "de_bytes")]
		Suffix(Bytes),
		#[serde(serialize_with = "ser_regex", deserialize_with = "de_regex")]
		Matches(fancy_regex::Regex),
		In(Vec<CompiledValue>),
		NotIn(Vec<CompiledValue>),
		Gt(i64),
		Gte(i64),
		Lt(i64),
		Lte(i64),
		Cidr(ipnet::IpNet),
	}

	impl From<&CompiledOperator> for OperatorShadow {
		fn from(op: &CompiledOperator) -> Self {
			match op {
				CompiledOperator::Equals(v) => Self::Equals(v.clone()),
				CompiledOperator::NotEquals(v) => Self::NotEquals(v.clone()),
				CompiledOperator::Contains(b) => Self::Contains(b.clone()),
				CompiledOperator::NotContains(b) => Self::NotContains(b.clone()),
				CompiledOperator::Prefix(b) => Self::Prefix(b.clone()),
				CompiledOperator::Suffix(b) => Self::Suffix(b.clone()),
				CompiledOperator::Matches(r) => {
					Self::Matches(fancy_regex::Regex::new(r.as_str()).expect("round-trippable"))
				}
				CompiledOperator::In(vs) => Self::In(vs.clone()),
				CompiledOperator::NotIn(vs) => Self::NotIn(vs.clone()),
				CompiledOperator::Gt(i) => Self::Gt(*i),
				CompiledOperator::Gte(i) => Self::Gte(*i),
				CompiledOperator::Lt(i) => Self::Lt(*i),
				CompiledOperator::Lte(i) => Self::Lte(*i),
				CompiledOperator::Cidr(n) => Self::Cidr(*n),
			}
		}
	}

	impl From<OperatorShadow> for CompiledOperator {
		fn from(op: OperatorShadow) -> Self {
			match op {
				OperatorShadow::Equals(v) => Self::Equals(v),
				OperatorShadow::NotEquals(v) => Self::NotEquals(v),
				OperatorShadow::Contains(b) => Self::Contains(b),
				OperatorShadow::NotContains(b) => Self::NotContains(b),
				OperatorShadow::Prefix(b) => Self::Prefix(b),
				OperatorShadow::Suffix(b) => Self::Suffix(b),
				OperatorShadow::Matches(r) => Self::Matches(r),
				OperatorShadow::In(vs) => Self::In(vs),
				OperatorShadow::NotIn(vs) => Self::NotIn(vs),
				OperatorShadow::Gt(i) => Self::Gt(i),
				OperatorShadow::Gte(i) => Self::Gte(i),
				OperatorShadow::Lt(i) => Self::Lt(i),
				OperatorShadow::Lte(i) => Self::Lte(i),
				OperatorShadow::Cidr(n) => Self::Cidr(n),
			}
		}
	}
}

impl serde::Serialize for CompiledValue {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		serde_impls::ValueShadow::from(self).serialize(s)
	}
}

impl<'de> serde::Deserialize<'de> for CompiledValue {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		serde_impls::ValueShadow::deserialize(d).map(Self::from)
	}
}

impl serde::Serialize for CompiledOperator {
	fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
		serde_impls::OperatorShadow::from(self).serialize(s)
	}
}

impl<'de> serde::Deserialize<'de> for CompiledOperator {
	fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
		serde_impls::OperatorShadow::deserialize(d).map(Self::from)
	}
}

#[cfg(test)]
mod tests {
	use std::collections::hash_map::DefaultHasher;
	use std::hash::Hash;
	use std::net::{Ipv4Addr, Ipv6Addr};
	use std::str::FromStr;
	use std::sync::OnceLock;
	use std::time::Instant;

	use bytes::Bytes;
	use fancy_regex::Regex;
	use ipnet::IpNet;
	use parking_lot::Mutex;

	use super::*;
	use crate::body::{Body, Request};
	use crate::conn_context::{ConnId, Transport};

	// Tests below cover `Hash` / `Eq` semantics and IR construction.
	// Behavior assertions for `PredicateInst::test` itself live alongside
	// the executor, where the `PredicateView` is exercised end-to-end.

	fn hash_of<T: Hash>(v: &T) -> u64 {
		let mut h = DefaultHasher::new();
		v.hash(&mut h);
		h.finish()
	}

	fn make_conn() -> Arc<ConnContext> {
		Arc::new(ConnContext {
			id: ConnId(1),
			remote: "127.0.0.1:0".parse().expect("parse remote"),
			local: "127.0.0.1:0".parse().expect("parse local"),
			transport: Transport::Tcp,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	#[test]
	fn field_path_http_header_is_equal_by_string_content_not_arc_identity() {
		let a = FieldPath::HttpHeader(Arc::from("host"));
		let b = FieldPath::HttpHeader(Arc::from("host"));
		assert_eq!(a, b);
		assert_eq!(hash_of(&a), hash_of(&b));
		// Arcs are distinct allocations; Hash/Eq must not depend on pointer
		// identity. Per the 18-predicate-schema grammar, path segments are
		// already lowercased upstream, so lower/upper comparison is a sanity
		// check that the compiled form does not re-casefold.
		let upper = FieldPath::HttpHeader(Arc::from("Host"));
		assert_ne!(a, upper);
	}

	#[test]
	fn field_path_simple_variants_are_self_equal_and_mutually_distinct() {
		let paths = [
			FieldPath::Transport,
			FieldPath::RemoteIp,
			FieldPath::RemotePort,
			FieldPath::LocalIp,
			FieldPath::LocalPort,
			FieldPath::Peek,
			FieldPath::TlsSni,
			FieldPath::TlsAlpn,
			FieldPath::TlsVersion,
			FieldPath::TlsPeerCertSubjectCn,
			FieldPath::HttpMethod,
			FieldPath::HttpUriPath,
			FieldPath::HttpUriQuery,
			FieldPath::HttpBody,
		];
		for (i, a) in paths.iter().enumerate() {
			for (j, b) in paths.iter().enumerate() {
				if i == j {
					assert_eq!(a, b);
				} else {
					assert_ne!(a, b);
				}
			}
		}
	}

	#[test]
	fn compiled_value_str_is_equal_by_content_not_arc_identity() {
		let a = CompiledValue::Str(Arc::<str>::from("x"));
		let b = CompiledValue::Str(Arc::<str>::from("x"));
		assert_eq!(a, b);
		assert_eq!(hash_of(&a), hash_of(&b));
		let c = CompiledValue::Str(Arc::<str>::from("y"));
		assert_ne!(a, c);
	}

	#[test]
	fn compiled_value_cross_variant_inequality() {
		let s = CompiledValue::Str(Arc::<str>::from("42"));
		let i = CompiledValue::Int(42);
		assert_ne!(s, i);
	}

	#[test]
	fn compiled_value_bytes_int_bool_addr_self_equal() {
		assert_eq!(
			CompiledValue::Bytes(Bytes::from_static(b"abc")),
			CompiledValue::Bytes(Bytes::copy_from_slice(b"abc")),
		);
		assert_eq!(CompiledValue::Int(7), CompiledValue::Int(7));
		assert_ne!(CompiledValue::Int(7), CompiledValue::Int(8));
		assert_eq!(CompiledValue::Bool(true), CompiledValue::Bool(true));
		assert_ne!(CompiledValue::Bool(true), CompiledValue::Bool(false));
		assert_eq!(
			CompiledValue::Addr(Ipv4Addr::new(10, 0, 0, 1).into()),
			CompiledValue::Addr(Ipv4Addr::new(10, 0, 0, 1).into()),
		);
		assert_ne!(
			CompiledValue::Addr(Ipv4Addr::new(10, 0, 0, 1).into()),
			CompiledValue::Addr(Ipv6Addr::LOCALHOST.into()),
		);
	}

	#[test]
	fn compiled_operator_matches_equal_by_pattern_source() {
		let a = CompiledOperator::Matches(Regex::new("^/api").expect("compile a"));
		let b = CompiledOperator::Matches(Regex::new("^/api").expect("compile b"));
		assert_eq!(a, b);
		assert_eq!(hash_of(&a), hash_of(&b));
	}

	#[test]
	fn compiled_operator_matches_distinct_patterns_unequal() {
		// Spec: the compiler does not rewrite regexes — structurally-different
		// but semantically-equivalent sources are treated as distinct.
		let a = CompiledOperator::Matches(Regex::new("a|b").expect("compile a"));
		let b = CompiledOperator::Matches(Regex::new("b|a").expect("compile b"));
		assert_ne!(a, b);
	}

	#[test]
	fn compiled_operator_cidr_equal_by_canonical_form() {
		let a = CompiledOperator::Cidr(IpNet::from_str("10.0.0.0/8").expect("parse a"));
		let b = CompiledOperator::Cidr(IpNet::from_str("10.0.0.0/8").expect("parse b"));
		assert_eq!(a, b);
		assert_eq!(hash_of(&a), hash_of(&b));
	}

	#[test]
	fn compiled_operator_cidr_distinct_networks_unequal() {
		let a = CompiledOperator::Cidr(IpNet::from_str("10.0.0.0/8").expect("parse a"));
		let b = CompiledOperator::Cidr(IpNet::from_str("10.0.0.0/16").expect("parse b"));
		assert_ne!(a, b);
	}

	#[test]
	fn compiled_operator_in_is_order_sensitive() {
		let xs =
			vec![CompiledValue::Str(Arc::<str>::from("a")), CompiledValue::Str(Arc::<str>::from("b"))];
		let ys =
			vec![CompiledValue::Str(Arc::<str>::from("b")), CompiledValue::Str(Arc::<str>::from("a"))];
		assert_ne!(CompiledOperator::In(xs.clone()), CompiledOperator::In(ys.clone()));
		assert_ne!(CompiledOperator::NotIn(xs), CompiledOperator::NotIn(ys));
	}

	#[test]
	fn compiled_operator_numeric_comparisons_distinct_per_variant() {
		// Gt / Gte / Lt / Lte with the same threshold are distinct operators.
		let ops = [
			CompiledOperator::Gt(10),
			CompiledOperator::Gte(10),
			CompiledOperator::Lt(10),
			CompiledOperator::Lte(10),
		];
		for (i, a) in ops.iter().enumerate() {
			for (j, b) in ops.iter().enumerate() {
				if i == j {
					assert_eq!(a, b);
				} else {
					assert_ne!(a, b);
				}
			}
		}
	}

	#[test]
	fn compiled_operator_bytes_variants_distinguished() {
		let payload = Bytes::from_static(b"abc");
		let ops = [
			CompiledOperator::Contains(payload.clone()),
			CompiledOperator::NotContains(payload.clone()),
			CompiledOperator::Prefix(payload.clone()),
			CompiledOperator::Suffix(payload),
		];
		for (i, a) in ops.iter().enumerate() {
			for (j, b) in ops.iter().enumerate() {
				if i == j {
					assert_eq!(a, b);
				} else {
					assert_ne!(a, b);
				}
			}
		}
	}

	#[test]
	fn predicate_inst_equal_across_independent_construction_paths() {
		let lhs = PredicateInst {
			path: FieldPath::HttpHeader(Arc::from("host")),
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from("example.com"))),
		};
		let rhs = PredicateInst {
			path: FieldPath::HttpHeader(Arc::from("host")),
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from("example.com"))),
		};
		assert_eq!(lhs, rhs);
		assert_eq!(hash_of(&lhs), hash_of(&rhs));
	}

	#[test]
	fn predicate_inst_equal_with_regex_operator_from_separate_compiles() {
		let lhs = PredicateInst {
			path: FieldPath::HttpUriPath,
			op: CompiledOperator::Matches(Regex::new("^/").expect("compile a")),
		};
		let rhs = PredicateInst {
			path: FieldPath::HttpUriPath,
			op: CompiledOperator::Matches(Regex::new("^/").expect("compile b")),
		};
		assert_eq!(lhs, rhs);
		assert_eq!(hash_of(&lhs), hash_of(&rhs));
	}

	#[test]
	fn predicate_inst_unequal_on_path_difference() {
		let value = CompiledValue::Str(Arc::<str>::from("x"));
		let a =
			PredicateInst { path: FieldPath::HttpUriPath, op: CompiledOperator::Equals(value.clone()) };
		let b = PredicateInst { path: FieldPath::HttpUriQuery, op: CompiledOperator::Equals(value) };
		assert_ne!(a, b);
	}

	#[test]
	fn predicate_view_variants_construct() {
		let conn = make_conn();
		let peek_bytes: &[u8] = b"\x16\x03\x01";
		let l4 = PredicateView::L4 { conn: &conn, peek: Some(peek_bytes) };
		match l4 {
			PredicateView::L4 { peek, .. } => assert_eq!(peek.map(<[u8]>::len), Some(3)),
			PredicateView::L7Req { .. } => panic!("wrong variant"),
		}

		let conn2 = make_conn();
		let req: Request =
			http::Request::builder().method("GET").uri("/").body(Body::Empty).expect("build request");
		let l7 = PredicateView::L7Req { conn: &conn2, req: &req };
		match l7 {
			PredicateView::L7Req { .. } => {}
			PredicateView::L4 { .. } => panic!("wrong variant"),
		}
	}

	// Parse-layer coverage for Predicate / CheckMap / Operator / Value.
	// Tests exercise the wire format defined in spec/crates/core.md.

	fn parse_predicate(v: serde_json::Value) -> Result<Predicate, serde_json::Error> {
		serde_json::from_value(v)
	}

	fn expect_check(p: &Predicate) -> &CheckMap {
		match p {
			Predicate::Check(c) => c,
			other => panic!("expected Predicate::Check, got {other:?}"),
		}
	}

	#[test]
	fn parse_any_of_happy_path() {
		let raw = serde_json::json!({
			"any_of": [
				{ "tls.sni": { "equals": "a" } },
				{ "tls.sni": { "equals": "b" } },
			],
		});
		let p = parse_predicate(raw).expect("parse any_of");
		let Predicate::AnyOf(AnyOfP { any_of }) = p else {
			panic!("expected AnyOf");
		};
		assert_eq!(any_of.len(), 2);
		let c0 = expect_check(&any_of[0]);
		let c1 = expect_check(&any_of[1]);
		assert_eq!(c0.path, FieldPath::TlsSni);
		assert_eq!(c1.path, FieldPath::TlsSni);
		match (&c0.op, &c1.op) {
			(Operator::Equals(Value::Str(a)), Operator::Equals(Value::Str(b))) => {
				assert_eq!(a, "a");
				assert_eq!(b, "b");
			}
			(a, b) => panic!("unexpected ops: {a:?} / {b:?}"),
		}
	}

	#[test]
	fn parse_not_happy_path() {
		let raw = serde_json::json!({
			"not": { "tls.sni": { "equals": "internal" } },
		});
		let p = parse_predicate(raw).expect("parse not");
		let Predicate::Not(NotP { not }) = p else {
			panic!("expected Not");
		};
		let inner = expect_check(&not);
		assert_eq!(inner.path, FieldPath::TlsSni);
		match &inner.op {
			Operator::Equals(Value::Str(s)) => assert_eq!(s, "internal"),
			other => panic!("unexpected op: {other:?}"),
		}
	}

	#[test]
	fn parse_all_of_happy_path() {
		let raw = serde_json::json!({
			"all_of": [
				{ "http.header.upgrade": { "equals": "websocket" } },
				{ "http.uri.path": { "prefix": "/ws" } },
			],
		});
		let p = parse_predicate(raw).expect("parse all_of");
		let Predicate::AllOf(AllOfP { all_of }) = p else {
			panic!("expected AllOf");
		};
		assert_eq!(all_of.len(), 2);
		let c0 = expect_check(&all_of[0]);
		let c1 = expect_check(&all_of[1]);
		assert_eq!(c0.path, FieldPath::HttpHeader(Arc::from("upgrade")));
		assert_eq!(c1.path, FieldPath::HttpUriPath);
	}

	#[test]
	fn parse_all_of_empty_array_parses() {
		// `all_of: []` is an empty conjunction — vacuously true. Parse must
		// succeed; the `lower` pass folds it to `on_match` directly.
		let raw = serde_json::json!({ "all_of": [] });
		let p = parse_predicate(raw).expect("empty all_of parses");
		let Predicate::AllOf(AllOfP { all_of }) = p else {
			panic!("expected AllOf");
		};
		assert!(all_of.is_empty());
	}

	#[test]
	fn parse_all_of_nested_with_check_and_any_of() {
		let raw = serde_json::json!({
			"all_of": [
				{ "tls.sni": { "equals": "api.example.com" } },
				{ "any_of": [
					{ "remote.ip": { "cidr": "10.0.0.0/8" } },
					{ "remote.ip": { "cidr": "192.168.0.0/16" } },
				]},
			],
		});
		let p = parse_predicate(raw).expect("parse nested all_of/any_of");
		let Predicate::AllOf(AllOfP { all_of }) = p else {
			panic!("expected AllOf");
		};
		assert_eq!(all_of.len(), 2);
		assert!(matches!(all_of[0], Predicate::Check(_)));
		assert!(matches!(all_of[1], Predicate::AnyOf(_)));
	}

	#[test]
	fn parse_all_of_with_extra_key_is_rejected() {
		// AllOfP carries deny_unknown_fields like AnyOfP.
		let raw = serde_json::json!({
			"all_of": [ { "tls.sni": { "equals": "a" } } ],
			"extra": "unwanted",
		});
		let err = parse_predicate(raw).expect_err("must reject extra key on all_of");
		let _ = err.to_string();
	}

	#[test]
	fn parse_http_header_all_of_is_a_check_not_combinator() {
		// A header literally named "all_of" parses as a Check via the
		// multi-segment dotted-path path, mirroring the AnyOf treatment.
		let raw = serde_json::json!({ "http.header.all_of": { "equals": "x" } });
		let p = parse_predicate(raw).expect("parse http.header.all_of");
		let c = expect_check(&p);
		assert_eq!(c.path, FieldPath::HttpHeader(Arc::from("all_of")));
	}

	#[test]
	fn parse_check_across_representative_paths() {
		let cases = [
			(serde_json::json!({ "tls.sni": { "equals": "api.example.com" } }), FieldPath::TlsSni),
			(serde_json::json!({ "remote.port": { "gt": 1024 } }), FieldPath::RemotePort),
			(serde_json::json!({ "http.method": { "equals": "GET" } }), FieldPath::HttpMethod),
			(serde_json::json!({ "http.uri.path": { "prefix": "/api" } }), FieldPath::HttpUriPath),
			(
				serde_json::json!({ "http.header.host": { "equals": "a.example.com" } }),
				FieldPath::HttpHeader(Arc::from("host")),
			),
			(serde_json::json!({ "http.body": { "contains": "hello" } }), FieldPath::HttpBody),
		];
		for (raw, expected_path) in cases {
			let p = parse_predicate(raw.clone()).unwrap_or_else(|e| panic!("parse {raw}: {e}"));
			let c = expect_check(&p);
			assert_eq!(c.path, expected_path, "input: {raw}");
		}
	}

	#[test]
	fn parse_any_of_with_extra_key_is_rejected() {
		// AnyOfP carries deny_unknown_fields; an object with any_of + an extra key must not
		// silently fall back to Check (two top-level keys would also fail CheckMap).
		let raw = serde_json::json!({
			"any_of": [ { "tls.sni": { "equals": "a" } } ],
			"extra": true,
		});
		let err = parse_predicate(raw).expect_err("must reject extra key on any_of");
		let _ = err.to_string();
	}

	#[test]
	fn parse_http_header_any_of_is_a_check_not_combinator() {
		// A header literally named "any_of" is a multi-segment dotted path and is a Check,
		// not the combinator form. spec/crates/core.md § "Why this doesn't need reserved-word policy".
		let raw = serde_json::json!({ "http.header.any_of": { "equals": "x" } });
		let p = parse_predicate(raw).expect("parse");
		let c = expect_check(&p);
		assert_eq!(c.path, FieldPath::HttpHeader(Arc::from("any_of")));
	}

	#[test]
	fn parse_uppercase_field_path_suggests_lowercase() {
		let raw = serde_json::json!({ "http.header.Host": { "equals": "x" } });
		let err = parse_predicate(raw).expect_err("uppercase must fail");
		let msg = err.to_string();
		assert!(msg.contains("http.header.Host"), "error mentions offending input: {msg}");
		assert!(msg.contains("did you mean"), "error includes suggestion phrase: {msg}");
		assert!(msg.contains("http.header.host"), "error contains lowercased form: {msg}");
	}

	#[test]
	fn parse_multi_key_check_is_rejected() {
		let raw = serde_json::json!({
			"http.uri.path": { "matches": "^/" },
			"http.method": { "equals": "GET" },
		});
		let err = parse_predicate(raw).expect_err("multi-key check must fail");
		let _ = err.to_string();
	}

	#[test]
	fn parse_empty_http_header_name_is_rejected() {
		let raw = serde_json::json!({ "http.header.": { "equals": "x" } });
		let err = parse_predicate(raw).expect_err("empty header name must fail");
		let _ = err.to_string();
	}

	#[test]
	fn parse_unknown_field_path_is_rejected_with_name() {
		let raw = serde_json::json!({ "http.nope": { "equals": "x" } });
		let err = parse_predicate(raw).expect_err("unknown path must fail");
		let msg = err.to_string();
		assert!(msg.contains("http.nope"), "error mentions offending path: {msg}");
	}

	fn parse_op(v: serde_json::Value) -> Operator {
		let mut map = serde_json::Map::new();
		map.insert("tls.sni".to_string(), v);
		let raw = serde_json::Value::Object(map);
		match parse_predicate(raw).expect("parse check") {
			Predicate::Check(c) => c.op,
			other => panic!("expected Check, got {other:?}"),
		}
	}

	#[test]
	fn operator_equals_and_not_equals_on_string() {
		let eq = parse_op(serde_json::json!({ "equals": "api" }));
		match eq {
			Operator::Equals(Value::Str(s)) => assert_eq!(s, "api"),
			other => panic!("expected equals/str: {other:?}"),
		}
		let neq = parse_op(serde_json::json!({ "not_equals": "api" }));
		match neq {
			Operator::NotEquals(Value::Str(s)) => assert_eq!(s, "api"),
			other => panic!("expected not_equals/str: {other:?}"),
		}
	}

	#[test]
	fn operator_contains_and_not_contains_on_string() {
		let c = parse_op(serde_json::json!({ "contains": "foo" }));
		match c {
			Operator::Contains(Value::Str(s)) => assert_eq!(s, "foo"),
			other => panic!("expected contains/str: {other:?}"),
		}
		let nc = parse_op(serde_json::json!({ "not_contains": "foo" }));
		match nc {
			Operator::NotContains(Value::Str(s)) => assert_eq!(s, "foo"),
			other => panic!("expected not_contains/str: {other:?}"),
		}
	}

	#[test]
	fn operator_prefix_and_suffix_on_string() {
		let p = parse_op(serde_json::json!({ "prefix": "/api" }));
		match p {
			Operator::Prefix(Value::Str(s)) => assert_eq!(s, "/api"),
			other => panic!("expected prefix/str: {other:?}"),
		}
		let s = parse_op(serde_json::json!({ "suffix": ".json" }));
		match s {
			Operator::Suffix(Value::Str(v)) => assert_eq!(v, ".json"),
			other => panic!("expected suffix/str: {other:?}"),
		}
	}

	#[test]
	fn operator_matches_carries_pattern_source() {
		let op = parse_op(serde_json::json!({ "matches": "^/api/v\\d+" }));
		match op {
			Operator::Matches(pattern) => assert_eq!(pattern, "^/api/v\\d+"),
			other => panic!("expected matches: {other:?}"),
		}
	}

	#[test]
	fn operator_in_and_not_in_accept_mixed_scalar_types() {
		let op = parse_op(serde_json::json!({ "in": ["foo", 42] }));
		let Operator::In(xs) = op else {
			panic!("expected in");
		};
		assert_eq!(xs.len(), 2);
		assert_eq!(xs[0], Value::Str("foo".into()));
		assert_eq!(xs[1], Value::Int(42));
		let op2 = parse_op(serde_json::json!({ "not_in": ["bar", 7] }));
		let Operator::NotIn(ys) = op2 else {
			panic!("expected not_in");
		};
		assert_eq!(ys.len(), 2);
		assert_eq!(ys[0], Value::Str("bar".into()));
		assert_eq!(ys[1], Value::Int(7));
	}

	#[test]
	fn operator_numeric_comparisons() {
		assert!(matches!(parse_op(serde_json::json!({ "gt": 10 })), Operator::Gt(10)));
		assert!(matches!(parse_op(serde_json::json!({ "gte": 10 })), Operator::Gte(10)));
		assert!(matches!(parse_op(serde_json::json!({ "lt": 10 })), Operator::Lt(10)));
		assert!(matches!(parse_op(serde_json::json!({ "lte": 10 })), Operator::Lte(10)));
	}

	#[test]
	fn operator_cidr_carries_source_string() {
		let op = parse_op(serde_json::json!({ "cidr": "10.0.0.0/8" }));
		match op {
			Operator::Cidr(s) => assert_eq!(s, "10.0.0.0/8"),
			other => panic!("expected cidr: {other:?}"),
		}
	}

	#[test]
	fn value_untagged_priority_bool_before_str() {
		// Per the untagged listing (Bool, Int, Str), `true`/`false` must land as Bool,
		// not as Str("true").
		let op_t = parse_op(serde_json::json!({ "equals": true }));
		assert!(matches!(op_t, Operator::Equals(Value::Bool(true))));
		let op_f = parse_op(serde_json::json!({ "equals": false }));
		assert!(matches!(op_f, Operator::Equals(Value::Bool(false))));
	}

	#[test]
	fn value_untagged_priority_int_before_str() {
		// A JSON number `42` must land as Int, not as Str("42").
		let op = parse_op(serde_json::json!({ "equals": 42 }));
		assert!(matches!(op, Operator::Equals(Value::Int(42))));
	}

	#[test]
	fn value_untagged_json_string_stays_str() {
		// A JSON string `"42"` must land as Str; the untagged enum must not coerce digit
		// strings into Int.
		let op = parse_op(serde_json::json!({ "equals": "42" }));
		match op {
			Operator::Equals(Value::Str(s)) => assert_eq!(s, "42"),
			other => panic!("expected equals/str(\"42\"): {other:?}"),
		}
	}

	#[test]
	fn regex_pattern_exactly_at_limit_parses() {
		// 4096 bytes == REGEX_PATTERN_MAX_BYTES; must parse.
		assert_eq!(REGEX_PATTERN_MAX_BYTES, 4 * 1024);
		let pattern = "a".repeat(REGEX_PATTERN_MAX_BYTES);
		let raw = serde_json::json!({ "http.uri.path": { "matches": pattern } });
		let p = parse_predicate(raw).expect("4 KiB pattern parses");
		let c = expect_check(&p);
		match &c.op {
			Operator::Matches(src) => assert_eq!(src.len(), REGEX_PATTERN_MAX_BYTES),
			other => panic!("expected matches: {other:?}"),
		}
	}

	#[test]
	fn regex_pattern_over_limit_rejected_with_limit_in_message() {
		let pattern = "a".repeat(REGEX_PATTERN_MAX_BYTES + 1);
		let raw = serde_json::json!({ "http.uri.path": { "matches": pattern } });
		let err = parse_predicate(raw).expect_err("over-limit pattern must fail");
		let msg = err.to_string();
		assert!(
			msg.contains(&REGEX_PATTERN_MAX_BYTES.to_string()),
			"error mentions the limit ({REGEX_PATTERN_MAX_BYTES}): {msg}",
		);
	}

	// ──────────────────────────────────────────────────────────────────────
	// Dry-run JSON wire-format contract (spec/flow-model.md § _The compiled form_).
	// The compiled IR round-trips through the shadow-enum convention
	// documented in spec: externally-tagged snake_case for both `FieldPath`
	// and `CompiledValue` / `CompiledOperator`, bytes as STANDARD base64,
	// regex as the source string, CIDR as canonical form.
	// ──────────────────────────────────────────────────────────────────────

	fn value_round_trip(v: &CompiledValue) -> CompiledValue {
		let encoded = serde_json::to_string(v).expect("serialize value");
		serde_json::from_str(&encoded).expect("deserialize value")
	}

	#[test]
	fn compiled_value_str_round_trip_including_empty() {
		let non_empty = CompiledValue::Str(Arc::<str>::from("x"));
		assert_eq!(value_round_trip(&non_empty), non_empty);
		let empty = CompiledValue::Str(Arc::<str>::from(""));
		assert_eq!(value_round_trip(&empty), empty);
	}

	#[test]
	fn compiled_value_bytes_round_trip_including_empty_and_binary() {
		let hello = CompiledValue::Bytes(Bytes::from_static(b"hello"));
		assert_eq!(value_round_trip(&hello), hello);
		let empty = CompiledValue::Bytes(Bytes::new());
		assert_eq!(value_round_trip(&empty), empty);
		let binary = CompiledValue::Bytes(Bytes::from_static(&[0xff, 0x00, 0x13]));
		assert_eq!(value_round_trip(&binary), binary);
	}

	#[test]
	fn compiled_value_int_round_trip_including_extremes() {
		for i in [0_i64, i64::MIN, i64::MAX] {
			let v = CompiledValue::Int(i);
			assert_eq!(value_round_trip(&v), v);
		}
	}

	#[test]
	fn compiled_value_bool_round_trip_both_variants() {
		for b in [true, false] {
			let v = CompiledValue::Bool(b);
			assert_eq!(value_round_trip(&v), v);
		}
	}

	#[test]
	fn compiled_value_addr_round_trip_v4_and_v6() {
		let v4 = CompiledValue::Addr(Ipv4Addr::LOCALHOST.into());
		assert_eq!(value_round_trip(&v4), v4);
		let v6 = CompiledValue::Addr(Ipv6Addr::LOCALHOST.into());
		assert_eq!(value_round_trip(&v6), v6);
	}

	#[test]
	fn compiled_value_bytes_emits_standard_base64_literal() {
		// STANDARD base64 ("hello" → "aGVsbG8="). Pins the alphabet choice per
		// spec/flow-model.md § _The compiled form_ — a url-safe switch would break
		// external dry-run consumers.
		let v = CompiledValue::Bytes(Bytes::from_static(b"hello"));
		let encoded = serde_json::to_string(&v).expect("serialize");
		assert_eq!(encoded, r#"{"bytes":"aGVsbG8="}"#);
	}

	fn op_round_trip(op: &CompiledOperator) -> CompiledOperator {
		let encoded = serde_json::to_string(op).expect("serialize op");
		serde_json::from_str(&encoded).expect("deserialize op")
	}

	#[test]
	fn compiled_operator_equals_and_not_equals_round_trip() {
		let eq = CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from("x")));
		assert_eq!(op_round_trip(&eq), eq);
		let neq = CompiledOperator::NotEquals(CompiledValue::Str(Arc::<str>::from("x")));
		assert_eq!(op_round_trip(&neq), neq);
	}

	#[test]
	fn compiled_operator_bytes_variants_round_trip() {
		let payload = Bytes::from_static(b"hello");
		let ops = [
			CompiledOperator::Contains(payload.clone()),
			CompiledOperator::NotContains(payload.clone()),
			CompiledOperator::Prefix(payload.clone()),
			CompiledOperator::Suffix(payload),
		];
		for op in ops {
			assert_eq!(op_round_trip(&op), op);
		}
	}

	#[test]
	fn compiled_operator_matches_round_trip_preserves_pattern_source() {
		let op = CompiledOperator::Matches(Regex::new("^/api/v[0-9]+").expect("compile"));
		let decoded = op_round_trip(&op);
		// Regex equality is by source (see `CompiledOperator::eq` above).
		assert_eq!(decoded, op);
		match decoded {
			CompiledOperator::Matches(r) => assert_eq!(r.as_str(), "^/api/v[0-9]+"),
			other => panic!("expected matches, got {other:?}"),
		}
	}

	#[test]
	fn compiled_operator_in_and_not_in_round_trip_mixed_values() {
		let xs = vec![CompiledValue::Str(Arc::<str>::from("a")), CompiledValue::Int(42)];
		let in_op = CompiledOperator::In(xs.clone());
		assert_eq!(op_round_trip(&in_op), in_op);
		let not_in_op = CompiledOperator::NotIn(xs);
		assert_eq!(op_round_trip(&not_in_op), not_in_op);
	}

	#[test]
	fn compiled_operator_numeric_comparisons_round_trip() {
		let ops = [
			CompiledOperator::Gt(100),
			CompiledOperator::Gte(100),
			CompiledOperator::Lt(100),
			CompiledOperator::Lte(100),
		];
		for op in ops {
			assert_eq!(op_round_trip(&op), op);
		}
	}

	#[test]
	fn compiled_operator_cidr_round_trip_preserves_canonical_form() {
		let op = CompiledOperator::Cidr(IpNet::from_str("10.0.0.0/8").expect("parse"));
		assert_eq!(op_round_trip(&op), op);
	}

	#[test]
	fn compiled_operator_matches_with_invalid_regex_is_rejected() {
		// An unterminated character class is a classic invalid regex. The
		// shadow-enum's custom error path surfaces the offending source in
		// the error message so operators can locate the bad rule.
		let raw = r#"{"matches":"["}"#;
		let err = serde_json::from_str::<CompiledOperator>(raw)
			.expect_err("invalid regex must fail to deserialize");
		let msg = err.to_string();
		assert!(msg.contains('['), "error mentions offending regex source: {msg}");
	}

	#[test]
	fn predicate_inst_pins_exact_wire_shape_for_http_header_equals() {
		let inst = PredicateInst {
			path: FieldPath::HttpHeader(Arc::from("host")),
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from("example.com"))),
		};
		let encoded = serde_json::to_string(&inst).expect("serialize");
		assert_eq!(encoded, r#"{"path":{"http_header":"host"},"op":{"equals":{"str":"example.com"}}}"#,);
		let decoded: PredicateInst = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, inst);
	}

	#[test]
	fn predicate_inst_round_trip_with_regex_operator() {
		let inst = PredicateInst {
			path: FieldPath::HttpUriPath,
			op: CompiledOperator::Matches(Regex::new("^/api").expect("compile")),
		};
		let encoded = serde_json::to_string(&inst).expect("serialize");
		let decoded: PredicateInst = serde_json::from_str(&encoded).expect("deserialize");
		assert_eq!(decoded, inst);
	}

	// --- PredicateInst::test matrix coverage ----------------------------------
	//
	// Pin the runtime evaluation of the three arms wired in C19's WS chunk:
	// HttpHeader/Equals, HttpUriPath/Equals, HttpUriPath/Prefix. These were
	// only indirectly covered by the WS e2e — explicit unit tests guard
	// against future regressions in `PredicateInst::test`'s match arms.

	fn http_header_equals(name: &str, value: &str) -> PredicateInst {
		PredicateInst {
			path: FieldPath::HttpHeader(Arc::from(name)),
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from(value))),
		}
	}

	fn http_uri_path_equals(value: &str) -> PredicateInst {
		PredicateInst {
			path: FieldPath::HttpUriPath,
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from(value))),
		}
	}

	fn http_uri_path_prefix(value: &str) -> PredicateInst {
		PredicateInst {
			path: FieldPath::HttpUriPath,
			op: CompiledOperator::Prefix(Bytes::copy_from_slice(value.as_bytes())),
		}
	}

	fn tls_sni_equals(value: &str) -> PredicateInst {
		PredicateInst {
			path: FieldPath::TlsSni,
			op: CompiledOperator::Equals(CompiledValue::Str(Arc::<str>::from(value))),
		}
	}

	fn conn_with_sni(sni: &str) -> Arc<ConnContext> {
		let conn = make_conn();
		*conn.tls.lock() = Some(crate::conn_context::TlsInfo {
			sni: Some(sni.to_string()),
			alpn: None,
			version: None,
			peer_cert: None,
			zero_rtt_used: false,
		});
		conn
	}

	fn req_with_header(name: &str, value: &str) -> Request {
		http::Request::builder()
			.method("GET")
			.uri("/")
			.header(name, value)
			.body(Body::Empty)
			.expect("build req")
	}

	fn req_with_uri(uri: &str) -> Request {
		http::Request::builder().method("GET").uri(uri).body(Body::Empty).expect("build req")
	}

	#[test]
	fn predicate_test_http_header_equals_matches_when_present_and_equal() {
		let conn = make_conn();
		let req = req_with_header("upgrade", "websocket");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(http_header_equals("upgrade", "websocket").test(&view));
	}

	#[test]
	fn predicate_test_http_header_equals_misses_when_header_absent() {
		let conn = make_conn();
		let req = req_with_header("host", "example.com");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!http_header_equals("upgrade", "websocket").test(&view));
	}

	#[test]
	fn predicate_test_http_header_equals_value_is_case_sensitive() {
		// RFC 9110 § 5.5: header values are opaque strings, comparison
		// is byte-exact. `WebSocket` (uppercase W, S) must NOT match
		// `websocket`. Operators wanting case-insensitive value comparison
		// use a regex with `(?i)…`.
		let conn = make_conn();
		let req = req_with_header("upgrade", "WebSocket");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!http_header_equals("upgrade", "websocket").test(&view));
	}

	#[test]
	fn predicate_test_http_header_equals_name_lookup_is_case_insensitive() {
		// RFC 9110 § 5.1: header NAMES are case-insensitive. The compiled
		// `FieldPath::HttpHeader(Arc<str>)` is already lowercased by
		// `parse_field_path`, and `HeaderMap::get` folds case on read,
		// so `Upgrade` in the request still matches the lowercased
		// `upgrade` in the predicate.
		let conn = make_conn();
		let req = req_with_header("Upgrade", "websocket");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(http_header_equals("upgrade", "websocket").test(&view));
	}

	#[test]
	fn predicate_test_http_header_equals_misses_on_l4_view() {
		// L4 view has no `Request`; header lookups can't fire. Sound by
		// default: the predicate misses rather than spuriously matching
		// or panicking.
		let conn = make_conn();
		let view = PredicateView::L4 { conn: &conn, peek: None };
		assert!(!http_header_equals("upgrade", "websocket").test(&view));
	}

	#[test]
	fn predicate_test_http_uri_path_equals_matches_exact() {
		let conn = make_conn();
		let req = req_with_uri("/api/v1/users");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(http_uri_path_equals("/api/v1/users").test(&view));
	}

	#[test]
	fn predicate_test_http_uri_path_equals_misses_on_substring() {
		// `Equals` is exact-match. `/api/v1` is a prefix of `/api/v1/users`
		// but not equal — the path-prefix middleware uses the dedicated
		// `Prefix` operator below.
		let conn = make_conn();
		let req = req_with_uri("/api/v1/users");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!http_uri_path_equals("/api").test(&view));
	}

	#[test]
	fn predicate_test_http_uri_path_prefix_matches_when_path_starts_with() {
		let conn = make_conn();
		let req = req_with_uri("/api/v1/users");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(http_uri_path_prefix("/api").test(&view));
	}

	#[test]
	fn predicate_test_http_uri_path_prefix_misses_when_no_prefix() {
		let conn = make_conn();
		let req = req_with_uri("/admin");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!http_uri_path_prefix("/api").test(&view));
	}

	#[test]
	fn predicate_test_tls_sni_equals_matches_when_set() {
		// SNI multi-cert routing relies on this arm: a rule that filters
		// `match: { tls.sni: { equals: "api.example.com" } }` should fire
		// when the listener's TLS handshake captured the matching SNI.
		let conn = conn_with_sni("api.example.com");
		let req = req_with_uri("/");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(tls_sni_equals("api.example.com").test(&view));
	}

	#[test]
	fn predicate_test_tls_sni_equals_misses_when_unset() {
		// Cleartext listener — `ConnContext.tls` is `None`. The predicate
		// must miss rather than spuriously match the empty SNI string.
		let conn = make_conn();
		let req = req_with_uri("/");
		let view = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!tls_sni_equals("api.example.com").test(&view));
	}

	#[test]
	fn predicate_test_tls_sni_equals_works_in_l4_view_too() {
		// `tls.sni`'s inspection level is L4-peek per
		// spec/crates/core.md; the predicate must work in both
		// `PredicateView::L4 { conn, peek }` and `L7Req { conn, .. }`
		// since both views carry `conn` and post-handshake SNI is
		// stored on `ConnContext.tls`.
		let conn = conn_with_sni("api.example.com");
		let view = PredicateView::L4 { conn: &conn, peek: None };
		assert!(tls_sni_equals("api.example.com").test(&view));
	}

	// ──────────────────────────────────────────────────────────────────────
	// Full operator × value-type matrix coverage. Each cell marked `yes` in
	// spec/crates/core.md § _Operator × value type
	// compatibility_ has a happy + miss test below. Field paths are picked
	// representatively per value type — string-family ops on tls.sni cover
	// every Str-typed path because the runtime reads them all via the same
	// `test_str` helper.
	// ──────────────────────────────────────────────────────────────────────

	fn pred(path: FieldPath, op: CompiledOperator) -> PredicateInst {
		PredicateInst { path, op }
	}

	fn str_val(s: &str) -> CompiledValue {
		CompiledValue::Str(Arc::<str>::from(s))
	}

	fn bytes_val(b: &[u8]) -> CompiledValue {
		CompiledValue::Bytes(Bytes::copy_from_slice(b))
	}

	fn b(b: &[u8]) -> Bytes {
		Bytes::copy_from_slice(b)
	}

	fn make_conn_with(remote: &str, local: &str) -> Arc<ConnContext> {
		Arc::new(ConnContext {
			id: ConnId(1),
			remote: remote.parse().expect("parse remote"),
			local: local.parse().expect("parse local"),
			transport: Transport::Tcp,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	fn make_conn_with_transport(t: Transport) -> Arc<ConnContext> {
		Arc::new(ConnContext {
			id: ConnId(1),
			remote: "127.0.0.1:0".parse().expect("remote"),
			local: "127.0.0.1:0".parse().expect("local"),
			transport: t,
			entered_at: Instant::now(),
			tls: Mutex::new(None),
			http_version: OnceLock::new(),
			user: Mutex::new(http::Extensions::new()),
		})
	}

	fn conn_with_tls_alpn(alpn: &[u8]) -> Arc<ConnContext> {
		let conn = make_conn();
		*conn.tls.lock() = Some(crate::conn_context::TlsInfo {
			sni: None,
			alpn: Some(alpn.to_vec()),
			version: None,
			peer_cert: None,
			zero_rtt_used: false,
		});
		conn
	}

	fn conn_with_tls_version(v: crate::conn_context::TlsVersion) -> Arc<ConnContext> {
		let conn = make_conn();
		*conn.tls.lock() = Some(crate::conn_context::TlsInfo {
			sni: None,
			alpn: None,
			version: Some(v),
			peer_cert: None,
			zero_rtt_used: false,
		});
		conn
	}

	// ── Equality family × every value type ────────────────────────────────

	#[test]
	fn matrix_equality_str_happy_and_miss() {
		// FieldPath::TlsSni; ops Equals/NotEquals/In/NotIn covered by Str helpers.
		let conn = conn_with_sni("api.example.com");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::TlsSni, CompiledOperator::Equals(str_val("api.example.com"))).test(&v));
		assert!(
			!pred(FieldPath::TlsSni, CompiledOperator::Equals(str_val("other.example.com"))).test(&v)
		);
		assert!(
			pred(FieldPath::TlsSni, CompiledOperator::NotEquals(str_val("other.example.com"))).test(&v)
		);
		assert!(
			!pred(FieldPath::TlsSni, CompiledOperator::NotEquals(str_val("api.example.com"))).test(&v)
		);
	}

	#[test]
	fn matrix_equality_bytes_happy_and_miss() {
		// FieldPath::TlsAlpn (Bytes-typed) with CompiledValue::Bytes.
		let conn = conn_with_tls_alpn(b"h2");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::Equals(bytes_val(b"h2"))).test(&v));
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::Equals(bytes_val(b"http/1.1"))).test(&v));
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::NotEquals(bytes_val(b"http/1.1"))).test(&v));
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::NotEquals(bytes_val(b"h2"))).test(&v));
	}

	#[test]
	fn matrix_equality_int_happy_and_miss() {
		let conn = make_conn_with("127.0.0.1:9090", "127.0.0.1:80");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::RemotePort, CompiledOperator::Equals(CompiledValue::Int(9090))).test(&v)
		);
		assert!(
			!pred(FieldPath::RemotePort, CompiledOperator::Equals(CompiledValue::Int(81))).test(&v)
		);
		assert!(
			pred(FieldPath::RemotePort, CompiledOperator::NotEquals(CompiledValue::Int(81))).test(&v)
		);
		assert!(
			!pred(FieldPath::RemotePort, CompiledOperator::NotEquals(CompiledValue::Int(9090))).test(&v)
		);
	}

	#[test]
	fn matrix_equality_addr_happy_and_miss() {
		let conn = make_conn_with("10.0.0.5:55555", "127.0.0.1:80");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let ten: std::net::IpAddr = "10.0.0.5".parse().unwrap();
		let other: std::net::IpAddr = "10.0.0.6".parse().unwrap();
		assert!(pred(FieldPath::RemoteIp, CompiledOperator::Equals(CompiledValue::Addr(ten))).test(&v));
		assert!(
			!pred(FieldPath::RemoteIp, CompiledOperator::Equals(CompiledValue::Addr(other))).test(&v)
		);
		assert!(
			pred(FieldPath::RemoteIp, CompiledOperator::NotEquals(CompiledValue::Addr(other))).test(&v)
		);
		assert!(
			!pred(FieldPath::RemoteIp, CompiledOperator::NotEquals(CompiledValue::Addr(ten))).test(&v)
		);
	}

	#[test]
	fn matrix_equality_enum_transport_happy_and_miss() {
		let tcp = make_conn_with_transport(Transport::Tcp);
		let udp = make_conn_with_transport(Transport::Udp);
		let v_tcp = PredicateView::L4 { conn: &tcp, peek: None };
		let v_udp = PredicateView::L4 { conn: &udp, peek: None };
		assert!(pred(FieldPath::Transport, CompiledOperator::Equals(str_val("tcp"))).test(&v_tcp));
		assert!(!pred(FieldPath::Transport, CompiledOperator::Equals(str_val("udp"))).test(&v_tcp));
		assert!(pred(FieldPath::Transport, CompiledOperator::Equals(str_val("udp"))).test(&v_udp));
	}

	#[test]
	fn matrix_equality_enum_tls_version_happy_and_miss() {
		let conn = conn_with_tls_version(crate::conn_context::TlsVersion::Tls13);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::TlsVersion, CompiledOperator::Equals(str_val("1.3"))).test(&v));
		assert!(!pred(FieldPath::TlsVersion, CompiledOperator::Equals(str_val("1.2"))).test(&v));
		assert!(pred(FieldPath::TlsVersion, CompiledOperator::NotEquals(str_val("1.2"))).test(&v));
	}

	#[test]
	fn matrix_equality_enum_tls_version_misses_when_absent() {
		// Cleartext listener — `tls` is None. equals must miss.
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(!pred(FieldPath::TlsVersion, CompiledOperator::Equals(str_val("1.3"))).test(&v));
		// not_equals also misses on absent state — sound by default.
		assert!(!pred(FieldPath::TlsVersion, CompiledOperator::NotEquals(str_val("1.3"))).test(&v));
	}

	#[test]
	fn matrix_equality_enum_http_method_happy_and_miss() {
		let conn = make_conn();
		let req = http::Request::builder().method("POST").uri("/").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpMethod, CompiledOperator::Equals(str_val("POST"))).test(&v));
		assert!(!pred(FieldPath::HttpMethod, CompiledOperator::Equals(str_val("GET"))).test(&v));
		assert!(pred(FieldPath::HttpMethod, CompiledOperator::NotEquals(str_val("GET"))).test(&v));
	}

	// ── InList family × every value type ───────────────────────────────────

	#[test]
	fn matrix_in_list_str_happy_and_miss() {
		let conn = conn_with_sni("api.example.com");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let list = vec![str_val("a.example.com"), str_val("api.example.com")];
		assert!(pred(FieldPath::TlsSni, CompiledOperator::In(list.clone())).test(&v));
		let list_miss = vec![str_val("a.example.com"), str_val("b.example.com")];
		assert!(!pred(FieldPath::TlsSni, CompiledOperator::In(list_miss.clone())).test(&v));
		assert!(pred(FieldPath::TlsSni, CompiledOperator::NotIn(list_miss)).test(&v));
		assert!(!pred(FieldPath::TlsSni, CompiledOperator::NotIn(list)).test(&v));
	}

	#[test]
	fn matrix_in_list_bytes_happy_and_miss() {
		let conn = conn_with_tls_alpn(b"h2");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let list = vec![bytes_val(b"http/1.1"), bytes_val(b"h2")];
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::In(list.clone())).test(&v));
		let list_miss = vec![bytes_val(b"http/1.0"), bytes_val(b"http/1.1")];
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::In(list_miss.clone())).test(&v));
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::NotIn(list_miss)).test(&v));
	}

	#[test]
	fn matrix_in_list_int_happy_and_miss() {
		let conn = make_conn_with("127.0.0.1:443", "127.0.0.1:80");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let in_list = vec![CompiledValue::Int(80), CompiledValue::Int(443)];
		assert!(pred(FieldPath::RemotePort, CompiledOperator::In(in_list.clone())).test(&v));
		let miss_list = vec![CompiledValue::Int(80), CompiledValue::Int(81)];
		assert!(!pred(FieldPath::RemotePort, CompiledOperator::In(miss_list.clone())).test(&v));
		assert!(pred(FieldPath::RemotePort, CompiledOperator::NotIn(miss_list)).test(&v));
	}

	#[test]
	fn matrix_in_list_addr_happy_and_miss_mixed_family() {
		let conn = make_conn_with("10.0.0.5:55555", "127.0.0.1:80");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let v4: std::net::IpAddr = "10.0.0.5".parse().unwrap();
		let v6: std::net::IpAddr = "::1".parse().unwrap();
		let list = vec![CompiledValue::Addr(v6), CompiledValue::Addr(v4)];
		assert!(pred(FieldPath::RemoteIp, CompiledOperator::In(list.clone())).test(&v));
		let miss = vec![CompiledValue::Addr(v6)];
		assert!(!pred(FieldPath::RemoteIp, CompiledOperator::In(miss.clone())).test(&v));
		assert!(pred(FieldPath::RemoteIp, CompiledOperator::NotIn(miss)).test(&v));
	}

	#[test]
	fn matrix_in_list_enum_transport_happy_and_miss() {
		let conn = make_conn_with_transport(Transport::Udp);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let list = vec![str_val("tcp"), str_val("udp")];
		assert!(pred(FieldPath::Transport, CompiledOperator::In(list)).test(&v));
		let miss = vec![str_val("tcp")];
		assert!(!pred(FieldPath::Transport, CompiledOperator::In(miss.clone())).test(&v));
		assert!(pred(FieldPath::Transport, CompiledOperator::NotIn(miss)).test(&v));
	}

	// ── StringSubstr family × Str/Bytes ────────────────────────────────────

	#[test]
	fn matrix_substring_on_str_happy_and_miss() {
		let conn = make_conn();
		let req =
			http::Request::builder().method("GET").uri("/api/v1/users").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpUriPath, CompiledOperator::Contains(b(b"/v1/"))).test(&v));
		assert!(!pred(FieldPath::HttpUriPath, CompiledOperator::Contains(b(b"/v2/"))).test(&v));
		assert!(pred(FieldPath::HttpUriPath, CompiledOperator::NotContains(b(b"/v2/"))).test(&v));
		assert!(!pred(FieldPath::HttpUriPath, CompiledOperator::NotContains(b(b"/v1/"))).test(&v));
	}

	#[test]
	fn matrix_substring_on_bytes_happy_and_miss() {
		let conn = conn_with_tls_alpn(b"http/1.1");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::Contains(b(b"/1."))).test(&v));
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::Contains(b(b"/2."))).test(&v));
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::NotContains(b(b"/2."))).test(&v));
	}

	// ── StringPrefSuf family × Str/Bytes ───────────────────────────────────

	#[test]
	fn matrix_prefix_suffix_on_str_happy_and_miss() {
		let conn = make_conn();
		let req =
			http::Request::builder().method("GET").uri("/api/file.json?q=1").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpUriPath, CompiledOperator::Prefix(b(b"/api"))).test(&v));
		assert!(!pred(FieldPath::HttpUriPath, CompiledOperator::Prefix(b(b"/admin"))).test(&v));
		assert!(pred(FieldPath::HttpUriPath, CompiledOperator::Suffix(b(b".json"))).test(&v));
		assert!(!pred(FieldPath::HttpUriPath, CompiledOperator::Suffix(b(b".html"))).test(&v));
	}

	#[test]
	fn matrix_prefix_suffix_on_bytes_happy_and_miss() {
		let conn = conn_with_tls_alpn(b"http/1.1");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::Prefix(b(b"http"))).test(&v));
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::Prefix(b(b"h2"))).test(&v));
		assert!(pred(FieldPath::TlsAlpn, CompiledOperator::Suffix(b(b"1.1"))).test(&v));
		assert!(!pred(FieldPath::TlsAlpn, CompiledOperator::Suffix(b(b"2.0"))).test(&v));
	}

	// ── RegexMatches × Str ─────────────────────────────────────────────────

	#[test]
	fn matrix_regex_matches_on_str_happy_and_miss() {
		let conn = make_conn();
		let req =
			http::Request::builder().method("GET").uri("/api/v3/orders").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		let re = Regex::new(r"^/api/v\d+/orders").expect("compile regex");
		assert!(pred(FieldPath::HttpUriPath, CompiledOperator::Matches(re)).test(&v));
		let re_miss = Regex::new(r"^/admin").expect("compile regex");
		assert!(!pred(FieldPath::HttpUriPath, CompiledOperator::Matches(re_miss)).test(&v));
	}

	#[test]
	fn matrix_regex_matches_on_header_happy_and_miss() {
		let conn = make_conn();
		let req = http::Request::builder()
			.method("GET")
			.uri("/")
			.header("user-agent", "Mozilla/5.0 (Macintosh; Intel)")
			.body(Body::Empty)
			.unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		let re = Regex::new(r"(?i)mozilla").expect("compile");
		assert!(
			pred(FieldPath::HttpHeader(Arc::from("user-agent")), CompiledOperator::Matches(re)).test(&v)
		);
		let re_miss = Regex::new(r"^curl").expect("compile");
		assert!(
			!pred(FieldPath::HttpHeader(Arc::from("user-agent")), CompiledOperator::Matches(re_miss))
				.test(&v)
		);
	}

	// ── NumericCmp × Int ───────────────────────────────────────────────────

	#[test]
	fn matrix_numeric_cmp_gt_gte_lt_lte_happy_and_miss() {
		let conn = make_conn_with("127.0.0.1:1024", "127.0.0.1:443");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		// Gt
		assert!(pred(FieldPath::RemotePort, CompiledOperator::Gt(1023)).test(&v));
		assert!(!pred(FieldPath::RemotePort, CompiledOperator::Gt(1024)).test(&v));
		// Gte
		assert!(pred(FieldPath::RemotePort, CompiledOperator::Gte(1024)).test(&v));
		assert!(!pred(FieldPath::RemotePort, CompiledOperator::Gte(1025)).test(&v));
		// Lt
		assert!(pred(FieldPath::RemotePort, CompiledOperator::Lt(1025)).test(&v));
		assert!(!pred(FieldPath::RemotePort, CompiledOperator::Lt(1024)).test(&v));
		// Lte
		assert!(pred(FieldPath::RemotePort, CompiledOperator::Lte(1024)).test(&v));
		assert!(!pred(FieldPath::RemotePort, CompiledOperator::Lte(1023)).test(&v));
	}

	#[test]
	fn matrix_numeric_cmp_local_port_too() {
		// Same family, exercise local.port to confirm both Int paths work.
		let conn = make_conn_with("127.0.0.1:0", "127.0.0.1:8443");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(pred(FieldPath::LocalPort, CompiledOperator::Gt(8000)).test(&v));
		assert!(!pred(FieldPath::LocalPort, CompiledOperator::Gt(9000)).test(&v));
	}

	// ── CidrMatch × IpAddr ─────────────────────────────────────────────────

	#[test]
	fn matrix_cidr_v4_happy_and_miss() {
		let conn = make_conn_with("10.0.5.7:0", "127.0.0.1:0");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let ten = IpNet::from_str("10.0.0.0/8").unwrap();
		let nineteen2 = IpNet::from_str("192.168.0.0/16").unwrap();
		assert!(pred(FieldPath::RemoteIp, CompiledOperator::Cidr(ten)).test(&v));
		assert!(!pred(FieldPath::RemoteIp, CompiledOperator::Cidr(nineteen2)).test(&v));
	}

	#[test]
	fn matrix_cidr_v6_happy_and_miss() {
		let conn = make_conn_with("[2001:db8::5]:0", "127.0.0.1:0");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let net = IpNet::from_str("2001:db8::/32").unwrap();
		let other = IpNet::from_str("2001:dead::/32").unwrap();
		assert!(pred(FieldPath::RemoteIp, CompiledOperator::Cidr(net)).test(&v));
		assert!(!pred(FieldPath::RemoteIp, CompiledOperator::Cidr(other)).test(&v));
	}

	#[test]
	fn matrix_cidr_v4_against_v6_addr_misses() {
		// Spec 18 § _CIDR specifics_: a single cidr matches only its family.
		let conn = make_conn_with("[2001:db8::5]:0", "127.0.0.1:0");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let v4 = IpNet::from_str("0.0.0.0/0").unwrap();
		assert!(!pred(FieldPath::RemoteIp, CompiledOperator::Cidr(v4)).test(&v));
	}

	// ── Field-coverage spotchecks (paths the helpers exercise but whose own
	//    reader path needs explicit coverage) ──────────────────────────────

	#[test]
	fn http_uri_query_reader_returns_empty_when_query_absent() {
		// Spec: `Request.uri().query().unwrap_or("")`. So `equals ""` matches
		// when there is no query.
		let conn = make_conn();
		let req = http::Request::builder().method("GET").uri("/no-q").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpUriQuery, CompiledOperator::Equals(str_val(""))).test(&v));
		assert!(!pred(FieldPath::HttpUriQuery, CompiledOperator::Equals(str_val("q=1"))).test(&v));
	}

	#[test]
	fn http_uri_query_reader_matches_present_query() {
		let conn = make_conn();
		let req = http::Request::builder().method("GET").uri("/x?a=1&b=2").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpUriQuery, CompiledOperator::Equals(str_val("a=1&b=2"))).test(&v));
		assert!(pred(FieldPath::HttpUriQuery, CompiledOperator::Contains(b(b"b=2"))).test(&v));
	}

	#[test]
	fn local_ip_reader_uses_local_socket() {
		let conn = make_conn_with("10.0.0.5:0", "127.0.0.1:8443");
		let v = PredicateView::L4 { conn: &conn, peek: None };
		let local: std::net::IpAddr = "127.0.0.1".parse().unwrap();
		assert!(
			pred(FieldPath::LocalIp, CompiledOperator::Equals(CompiledValue::Addr(local))).test(&v)
		);
	}

	#[test]
	fn http_header_lookup_misses_for_non_utf8_value() {
		// HeaderValue::from_bytes accepts non-UTF-8 bytes; `to_str()` then
		// errors. The reader must miss rather than panic.
		let conn = make_conn();
		let bad =
			http::HeaderValue::from_bytes(&[0xff, 0xfe, 0xfd]).expect("non-utf8 header value parses");
		let mut builder = http::Request::builder().method("GET").uri("/");
		builder.headers_mut().expect("headers").insert("x-bad", bad);
		let req: Request = builder.body(Body::Empty).expect("build request");
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(
			!pred(
				FieldPath::HttpHeader(Arc::from("x-bad")),
				CompiledOperator::Equals(str_val("anything")),
			)
			.test(&v)
		);
	}

	// ── tls.peer_cert.subject_cn (Str-typed) ──────────────────────────────

	fn rcgen_cert_with_cn(cn: &str) -> rustls_pki_types::CertificateDer<'static> {
		let mut params = rcgen::CertificateParams::default();
		params.distinguished_name = rcgen::DistinguishedName::new();
		params.distinguished_name.push(rcgen::DnType::CommonName, cn);
		let key = rcgen::KeyPair::generate().expect("rcgen keypair");
		let cert = params.self_signed(&key).expect("self-sign cert");
		cert.der().clone()
	}

	fn rcgen_cert_no_cn() -> rustls_pki_types::CertificateDer<'static> {
		// Build a cert whose Subject DN is empty (no CN). x509-parser
		// then returns no CommonName attribute; the reader must miss.
		let params = rcgen::CertificateParams::default();
		// Default DistinguishedName from rcgen actually carries a default
		// CN, so we replace it with an empty DN explicitly.
		let mut params = params;
		params.distinguished_name = rcgen::DistinguishedName::new();
		let key = rcgen::KeyPair::generate().expect("rcgen keypair");
		let cert = params.self_signed(&key).expect("self-sign cert");
		cert.der().clone()
	}

	fn conn_with_peer_cert(cert: &rustls_pki_types::CertificateDer<'static>) -> Arc<ConnContext> {
		let pc = crate::conn_context::PeerCertificate::from_der(cert)
			.expect("rcgen-issued cert must parse via PeerCertificate::from_der");
		let conn = make_conn();
		*conn.tls.lock() = Some(crate::conn_context::TlsInfo {
			sni: None,
			alpn: None,
			version: None,
			peer_cert: Some(Arc::new(pc)),
			zero_rtt_used: false,
		});
		conn
	}

	#[test]
	fn peer_cert_from_der_extracts_cn() {
		let cert = rcgen_cert_with_cn("client.internal");
		let pc = crate::conn_context::PeerCertificate::from_der(&cert).expect("parse");
		assert_eq!(pc.subject_cn.as_deref(), Some("client.internal"));
	}

	#[test]
	fn peer_cert_from_der_returns_none_for_malformed_der() {
		let raw = rustls_pki_types::CertificateDer::from(vec![0x30, 0x80, 0x00, 0x00]);
		assert!(crate::conn_context::PeerCertificate::from_der(&raw).is_none());
		let raw = rustls_pki_types::CertificateDer::from(b"not a cert at all".to_vec());
		assert!(crate::conn_context::PeerCertificate::from_der(&raw).is_none());
	}

	#[test]
	fn peer_cert_from_der_returns_some_with_no_cn_when_dn_has_no_cn() {
		// Empty-DN cert still parses; the CN field is absent.
		let cert = rcgen_cert_no_cn();
		let pc = crate::conn_context::PeerCertificate::from_der(&cert).expect("parse");
		assert!(pc.subject_cn.is_none());
	}

	#[test]
	fn matrix_peer_cert_subject_cn_equals_happy_and_miss() {
		let cert = rcgen_cert_with_cn("ops-bot");
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Equals(str_val("ops-bot"))).test(&v)
		);
		assert!(
			!pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Equals(str_val("attacker")))
				.test(&v)
		);
	}

	#[test]
	fn matrix_peer_cert_subject_cn_string_ops_happy_and_miss() {
		let cert = rcgen_cert_with_cn("svc-payments-prod");
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		// Prefix
		assert!(pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Prefix(b(b"svc-"))).test(&v));
		assert!(
			!pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Prefix(b(b"client-"))).test(&v)
		);
		// Suffix
		assert!(pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Suffix(b(b"-prod"))).test(&v));
		// Contains
		assert!(
			pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Contains(b(b"payments"))).test(&v)
		);
		// Matches
		let re = Regex::new(r"^svc-[a-z]+-(prod|stg)$").expect("regex");
		assert!(pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Matches(re)).test(&v));
		// In-list
		let list = vec![str_val("svc-other-prod"), str_val("svc-payments-prod")];
		assert!(pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::In(list)).test(&v));
	}

	#[test]
	fn peer_cert_subject_cn_misses_when_cert_absent() {
		// Cleartext or no-mTLS handshake: tls.peer_cert is None. Reader
		// must miss instead of panicking on missing state.
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			!pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Equals(str_val("anything")))
				.test(&v)
		);
	}

	#[test]
	fn peer_cert_subject_cn_misses_when_cert_has_no_cn() {
		// Sound-by-default for certs whose Subject DN omits CN entirely
		// (e.g. modern profile that puts identity in subjectAltName).
		let cert = rcgen_cert_no_cn();
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			!pred(FieldPath::TlsPeerCertSubjectCn, CompiledOperator::Equals(str_val("ops-bot"))).test(&v)
		);
	}

	// ── tls.peer_cert.* — new fields ──────────────────────────────────────

	fn rcgen_cert_with_san_dns(cn: &str, dns: &[&str]) -> rustls_pki_types::CertificateDer<'static> {
		let san: Vec<String> = dns.iter().map(|s| (*s).to_owned()).collect();
		let mut params = rcgen::CertificateParams::new(san).expect("rcgen params");
		params.distinguished_name = rcgen::DistinguishedName::new();
		params.distinguished_name.push(rcgen::DnType::CommonName, cn);
		let key = rcgen::KeyPair::generate().expect("rcgen keypair");
		let cert = params.self_signed(&key).expect("self-sign cert");
		cert.der().clone()
	}

	#[test]
	fn each_new_field_path_parses_from_string_form() {
		use super::parse_field_path;
		assert_eq!(parse_field_path("tls.peer_cert.present"), Ok(FieldPath::TlsPeerCertPresent));
		assert_eq!(parse_field_path("tls.peer_cert.san_dns"), Ok(FieldPath::TlsPeerCertSanDns));
		assert_eq!(
			parse_field_path("tls.peer_cert.fingerprint_sha256"),
			Ok(FieldPath::TlsPeerCertFingerprintSha256),
		);
		assert_eq!(parse_field_path("tls.peer_cert.spki_sha256"), Ok(FieldPath::TlsPeerCertSpkiSha256),);
		assert_eq!(parse_field_path("tls.peer_cert.issuer_cn"), Ok(FieldPath::TlsPeerCertIssuerCn));
		assert_eq!(parse_field_path("tls.peer_cert.serial"), Ok(FieldPath::TlsPeerCertSerial));
	}

	#[test]
	fn peer_cert_present_true_when_cert_attached() {
		let cert = rcgen_cert_with_cn("client.internal");
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::TlsPeerCertPresent, CompiledOperator::Equals(CompiledValue::Bool(true)))
				.test(&v)
		);
		assert!(
			!pred(FieldPath::TlsPeerCertPresent, CompiledOperator::Equals(CompiledValue::Bool(false)))
				.test(&v)
		);
	}

	#[test]
	fn peer_cert_present_false_when_cert_absent() {
		// Request-mode pattern: rule with `tls.peer_cert.present == false`
		// matches when the client did not present a cert.
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::TlsPeerCertPresent, CompiledOperator::Equals(CompiledValue::Bool(false)))
				.test(&v)
		);
		assert!(
			!pred(FieldPath::TlsPeerCertPresent, CompiledOperator::Equals(CompiledValue::Bool(true)))
				.test(&v)
		);
	}

	#[test]
	fn peer_cert_san_dns_contains_matches_listed_element() {
		let cert = rcgen_cert_with_san_dns("svc-a", &["svc-a.internal", "svc-b.internal"]);
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::TlsPeerCertSanDns, CompiledOperator::Contains(b(b"svc-a.internal"))).test(&v)
		);
		assert!(
			!pred(FieldPath::TlsPeerCertSanDns, CompiledOperator::Contains(b(b"svc-c.internal")))
				.test(&v),
		);
		assert!(
			pred(FieldPath::TlsPeerCertSanDns, CompiledOperator::NotContains(b(b"svc-c.internal")))
				.test(&v),
		);
	}

	#[test]
	fn peer_cert_san_dns_misses_when_cert_absent() {
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			!pred(FieldPath::TlsPeerCertSanDns, CompiledOperator::Contains(b(b"anything"))).test(&v)
		);
	}

	#[test]
	fn peer_cert_fingerprint_sha256_is_lowercase_hex_of_full_der() {
		use sha2::{Digest, Sha256};
		let cert = rcgen_cert_with_cn("fingerprinted");
		let mut h = Sha256::new();
		h.update(cert.as_ref());
		let want = h.finalize().iter().fold(String::new(), |mut s, b| {
			use std::fmt::Write as _;
			let _ = write!(s, "{b:02x}");
			s
		});

		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(
			pred(FieldPath::TlsPeerCertFingerprintSha256, CompiledOperator::Equals(str_val(&want)),)
				.test(&v),
		);
	}

	#[test]
	fn peer_cert_issuer_and_serial_present_for_self_signed_cert() {
		// rcgen self-signed: issuer == subject. Serial is rcgen-assigned;
		// we just check it's a non-empty lowercase-hex string.
		let cert = rcgen_cert_with_cn("issuer-test");
		let conn = conn_with_peer_cert(&cert);
		let v = PredicateView::L4 { conn: &conn, peek: None };
		// issuer_cn should equal subject for self-signed
		assert!(
			pred(FieldPath::TlsPeerCertIssuerCn, CompiledOperator::Equals(str_val("issuer-test")))
				.test(&v)
		);
		// Serial is non-empty hex (we don't know the exact value rcgen
		// picks; check shape via prefix-based contains using the empty
		// prefix as a proxy for "string is set").
		let pc = conn.tls.lock().as_ref().unwrap().peer_cert.as_ref().unwrap().clone();
		assert!(!pc.serial.is_empty(), "serial extracted");
		assert!(pc.serial.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
	}

	#[test]
	fn peer_cert_present_value_type_is_bool() {
		assert_eq!(FieldPath::TlsPeerCertPresent.value_type(), FieldValueType::Bool);
	}

	#[test]
	fn peer_cert_san_dns_value_type_is_vec_str() {
		assert_eq!(FieldPath::TlsPeerCertSanDns.value_type(), FieldValueType::VecStr);
	}

	#[test]
	fn matrix_rejects_string_pref_suf_on_bool_field() {
		// Bool accepts only equals / not_equals; prefix / suffix /
		// matches / contains all matrix-reject.
		assert!(!OperatorFamily::StringPrefSuf.accepts(FieldValueType::Bool));
		assert!(!OperatorFamily::StringSubstr.accepts(FieldValueType::Bool));
		assert!(!OperatorFamily::RegexMatches.accepts(FieldValueType::Bool));
		// equals is the only legal family on Bool
		assert!(OperatorFamily::Equality.accepts(FieldValueType::Bool));
	}

	#[test]
	fn matrix_rejects_equals_on_vec_str_field() {
		// Vec<Str> only accepts contains / not_contains. Equality and
		// regex / numeric / cidr all matrix-reject.
		assert!(!OperatorFamily::Equality.accepts(FieldValueType::VecStr));
		assert!(!OperatorFamily::InList.accepts(FieldValueType::VecStr));
		assert!(!OperatorFamily::StringPrefSuf.accepts(FieldValueType::VecStr));
		assert!(!OperatorFamily::RegexMatches.accepts(FieldValueType::VecStr));
		assert!(OperatorFamily::StringSubstr.accepts(FieldValueType::VecStr));
	}

	// ── http.body (Bytes-typed) ──────────────────────────────────────────
	//
	// Spec 18 § _Runtime_: the executor collects request body via
	// LazyBuffer before walking a Check on `http.body`, so by the time
	// the dispatch fires the body is `Body::Static(bytes)`. The tests
	// hand-build `Body::Static` directly to skip the LazyBuffer chain.

	fn req_with_body(body_bytes: &[u8]) -> Request {
		http::Request::builder()
			.method("POST")
			.uri("/upload")
			.body(Body::Static(Bytes::copy_from_slice(body_bytes)))
			.expect("build req with body")
	}

	#[test]
	fn matrix_http_body_equality_happy_and_miss() {
		let conn = make_conn();
		let req = req_with_body(b"hello world");
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(
			pred(FieldPath::HttpBody, CompiledOperator::Equals(bytes_val(b"hello world"))).test(&v)
		);
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::Equals(bytes_val(b"wrong"))).test(&v));
		assert!(pred(FieldPath::HttpBody, CompiledOperator::NotEquals(bytes_val(b"wrong"))).test(&v));
	}

	#[test]
	fn matrix_http_body_substring_happy_and_miss() {
		let conn = make_conn();
		let req = req_with_body(b"prelude payload trailer");
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpBody, CompiledOperator::Contains(b(b"payload"))).test(&v));
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::Contains(b(b"missing"))).test(&v));
		assert!(pred(FieldPath::HttpBody, CompiledOperator::NotContains(b(b"missing"))).test(&v));
	}

	#[test]
	fn matrix_http_body_prefix_suffix_happy_and_miss() {
		let conn = make_conn();
		let req = req_with_body(b"START middle END");
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(pred(FieldPath::HttpBody, CompiledOperator::Prefix(b(b"START"))).test(&v));
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::Prefix(b(b"BEGIN"))).test(&v));
		assert!(pred(FieldPath::HttpBody, CompiledOperator::Suffix(b(b"END"))).test(&v));
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::Suffix(b(b"FIN"))).test(&v));
	}

	#[test]
	fn matrix_http_body_in_list_happy_and_miss() {
		let conn = make_conn();
		let req = req_with_body(b"one");
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		let list = vec![bytes_val(b"two"), bytes_val(b"one")];
		assert!(pred(FieldPath::HttpBody, CompiledOperator::In(list)).test(&v));
		let miss = vec![bytes_val(b"two"), bytes_val(b"three")];
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::In(miss.clone())).test(&v));
		assert!(pred(FieldPath::HttpBody, CompiledOperator::NotIn(miss)).test(&v));
	}

	#[test]
	fn http_body_misses_on_l4_view() {
		// L4 view has no `Request`; sound-by-default miss instead of
		// panicking on the lazy-buffer invariant.
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(!pred(FieldPath::HttpBody, CompiledOperator::Contains(b(b"x"))).test(&v));
	}

	#[test]
	#[should_panic(expected = "lazy-buffer invariant")]
	fn http_body_panics_when_lazy_buffer_invariant_violated() {
		// Spec invariant: the executor MUST collect the request body
		// before reaching a Check on `http.body`. If a caller hands the
		// dispatch a `Body::Empty` (or `Body::Stream`) the predicate
		// path-reader trips `.expect("lazy-buffer invariant")`. This is
		// load-bearing: it surfaces FlowGraph compile bugs (forgotten
		// `collect_body_before` mark) as a clear panic instead of a
		// silent miss.
		let conn = make_conn();
		let req = http::Request::builder().method("POST").uri("/").body(Body::Empty).unwrap();
		let v = PredicateView::L7Req { conn: &conn, req: &req };
		let _ = pred(FieldPath::HttpBody, CompiledOperator::Contains(b(b"x"))).test(&v);
	}

	// ── peek (Bytes-typed) ───────────────────────────────────────────────
	//
	// `peek` reads the buffered ClientHello bytes captured by
	// `protocol_detect` before the L4→L7 upgrade. The reader returns
	// `false` when the buffer slot on the L4 view is `None` (already
	// covered above). When the slot is `Some(...)`, the bytes-family
	// operators apply.

	#[test]
	fn matrix_peek_substring_happy_and_miss() {
		// TLS ClientHello opens with handshake type 0x16, version 0x0301.
		let buf: &[u8] = &[0x16, 0x03, 0x01, 0x00, 0x40, 0x01];
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: Some(buf) };
		assert!(pred(FieldPath::Peek, CompiledOperator::Prefix(b(b"\x16\x03"))).test(&v));
		assert!(!pred(FieldPath::Peek, CompiledOperator::Prefix(b(b"\x14\x03"))).test(&v));
		assert!(pred(FieldPath::Peek, CompiledOperator::Contains(b(b"\x03\x01"))).test(&v));
		assert!(!pred(FieldPath::Peek, CompiledOperator::Contains(b(b"\xff\xff"))).test(&v));
	}

	#[test]
	fn matrix_peek_equality_happy_and_miss() {
		let buf: &[u8] = b"GET";
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: Some(buf) };
		assert!(pred(FieldPath::Peek, CompiledOperator::Equals(bytes_val(b"GET"))).test(&v));
		assert!(!pred(FieldPath::Peek, CompiledOperator::Equals(bytes_val(b"PUT"))).test(&v));
		assert!(pred(FieldPath::Peek, CompiledOperator::NotEquals(bytes_val(b"PUT"))).test(&v));
	}

	#[test]
	fn matrix_peek_in_list_happy_and_miss() {
		let buf: &[u8] = b"PRI ";
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: Some(buf) };
		// HTTP/2 prior-knowledge magic prefix begins with "PRI ".
		let list = vec![bytes_val(b"GET "), bytes_val(b"PRI ")];
		assert!(pred(FieldPath::Peek, CompiledOperator::In(list)).test(&v));
		let miss = vec![bytes_val(b"POST"), bytes_val(b"HEAD")];
		assert!(!pred(FieldPath::Peek, CompiledOperator::In(miss.clone())).test(&v));
		assert!(pred(FieldPath::Peek, CompiledOperator::NotIn(miss)).test(&v));
	}

	#[test]
	fn peek_misses_when_buffer_absent_on_l4_view() {
		// When peek slot is None (cleartext listener pre-protocol_detect,
		// or L7Req view), the reader must miss rather than panic.
		let conn = make_conn();
		let v = PredicateView::L4 { conn: &conn, peek: None };
		assert!(!pred(FieldPath::Peek, CompiledOperator::Prefix(b(b"\x16"))).test(&v));
		// Also confirm an L7Req view can never satisfy a peek predicate.
		let req = http::Request::builder().method("GET").uri("/").body(Body::Empty).unwrap();
		let v7 = PredicateView::L7Req { conn: &conn, req: &req };
		assert!(!pred(FieldPath::Peek, CompiledOperator::Prefix(b(b"\x16"))).test(&v7));
	}
}
