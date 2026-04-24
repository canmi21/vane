use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::Arc;

use bytes::Bytes;
use ipnet::IpNet;

use crate::body::Request;
use crate::conn_context::ConnContext;

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
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
	TlsPeerCertSubjectCn,
	HttpMethod,
	HttpUriPath,
	HttpUriQuery,
	HttpHeader(Arc<str>),
	HttpBody,
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

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PredicateInst {
	pub path: FieldPath,
	pub op: CompiledOperator,
}

pub enum PredicateView<'a> {
	L4 { conn: &'a Arc<ConnContext>, peek: Option<&'a [u8]> },
	L7Req { conn: &'a Arc<ConnContext>, req: &'a Request },
}

impl PredicateInst {
	#[must_use]
	pub fn test(&self, _view: &PredicateView<'_>) -> bool {
		// Dispatch lands with the lower pass in S1-09 (C5); that's where the
		// operator-by-value-type matrix in 18-predicate-schema.md gets wired
		// up against the same field-path readers it uses at compile time.
		todo!("PredicateInst::test lands with the lower pass in S1-09")
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

	// PredicateInst::test is todo!() until S1-09; behavior assertions live there.
	// Tests below cover Hash/Eq semantics and IR construction only.

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
}
