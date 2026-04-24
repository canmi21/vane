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
