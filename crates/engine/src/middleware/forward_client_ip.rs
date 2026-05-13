//! `forward_client_ip` — inject the L4 peer's identity into the
//! outgoing request as `X-Forwarded-For`, `X-Real-IP`, and / or RFC
//! 7239 `Forwarded`, **subject to a trusted-proxies allowlist**.
//!
//! Off by default at the raw-rule layer. The `reverse_proxy` preset
//! (`spec/crates/core.md`) inserts this middleware automatically
//! with the default header set.
//!
//! ## Trust model
//!
//! Inbound `X-Forwarded-For` / `Forwarded:` chains are only honoured
//! when the **L4 peer IP** (`conn.remote.ip()`) is a member of
//! `trusted_proxies`. Without trust, the inbound chain is replaced
//! wholesale with vane's observation. Default `trusted_proxies = []`
//! (no peer is trusted), which means the middleware always
//! overwrites — the safest baseline for an internet-facing edge
//! proxy.
//!
//! The `reverse_proxy` preset overrides this default with the
//! RFC1918 + ULA ranges, matching the most common
//! reverse-proxy-behind-LAN deployment.
//!
//! ## Headers written (per `headers`)
//!
//! - `X-Forwarded-For` — append-or-overwrite depending on trust.
//!   Honours the existing chain when peer is trusted, otherwise
//!   replaces with the bare peer IP.
//! - `X-Real-IP` — always overwrite with the L4 peer IP. The
//!   middleware never honours an inbound `X-Real-IP`; the daemon's
//!   L4 peer is the authoritative observation.
//! - `Forwarded` — RFC 7239. Emits `for=<peer>;by=<local>;proto=<https|http>`.
//!   IPv6 addresses go through the `"[…]"` quoted form (§4: "An IPv6
//!   address … MUST be enclosed within square brackets and quoted").
//!   Honours the inbound chain when peer is trusted, otherwise
//!   replaces with vane's bare observation.
//!
//! When `strip_inbound_forwarded` is enabled (default), the
//! middleware also removes inbound `X-Forwarded-Proto`,
//! `X-Forwarded-Host`, and any inbound `Forwarded` /
//! `X-Forwarded-For` that survived the trust check, so the upstream
//! never sees a half-honoured chain.
//!
//! Always returns `Decision::Continue` — this middleware never
//! short-circuits.

use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use ipnet::IpNet;
use vane_core::{
	ConnContext, Decision, Error, FlowCtx, L7RequestMiddleware, MiddlewareKind, Request,
};

use crate::factories::{FactoryError, MiddlewareFactories};
use crate::flow_graph::MiddlewareInst;

/// Per-header injection strategy. Decided at factory time so `run()`
/// is a flat dispatch on the enum tag, not a string compare per
/// request.
enum HeaderAction {
	XForwardedForAppend,
	XRealIpOverwrite,
	ForwardedAppend,
}

pub struct ForwardClientIp {
	actions: Vec<HeaderAction>,
	/// CIDR ranges whose L4 peer is allowed to dictate inbound
	/// `X-Forwarded-For` / `Forwarded:` chains. Empty = never trust
	/// inbound chains.
	trusted_proxies: Vec<IpNet>,
	/// When true (default), the middleware removes inbound
	/// `Forwarded`, `X-Forwarded-Proto`, and `X-Forwarded-Host`
	/// before writing its own values, so a half-honoured chain does
	/// not reach the upstream.
	strip_inbound_forwarded: bool,
}

impl ForwardClientIp {
	fn peer_is_trusted(&self, peer: IpAddr) -> bool {
		self.trusted_proxies.iter().any(|net| net.contains(&peer))
	}
}

#[async_trait]
impl L7RequestMiddleware for ForwardClientIp {
	async fn run(
		&self,
		req: &mut Request,
		conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		let peer = conn.remote.ip();
		let trusted = self.peer_is_trusted(peer);
		let proto = if conn.tls.lock().is_some() { "https" } else { "http" };
		let local = conn.local.ip();

		// Strip pre-existing forwarded headers we plan to ignore.
		// `strip_inbound_forwarded` removes both the bystander
		// X-Forwarded-{Proto,Host} (we don't honour them, so leaving
		// them around lets the upstream make a confused decision)
		// and the inbound `Forwarded:` itself when the peer is not
		// trusted (we re-add our own in the action loop below).
		if self.strip_inbound_forwarded {
			req.headers_mut().remove("x-forwarded-proto");
			req.headers_mut().remove("x-forwarded-host");
			if !trusted {
				req.headers_mut().remove("forwarded");
			}
		}
		if !trusted {
			// Untrusted peer: any existing `X-Forwarded-For` is the
			// attacker's claim. Drop it before re-writing — the
			// action loop will write a fresh value containing only
			// the peer IP.
			req.headers_mut().remove("x-forwarded-for");
		}

		let client_ip = peer.to_string();
		for action in &self.actions {
			match action {
				HeaderAction::XForwardedForAppend => {
					let name = http::header::HeaderName::from_static("x-forwarded-for");
					// At this point, trusted=true means we kept the
					// inbound `X-Forwarded-For`; trusted=false means
					// we cleared it above. Either way the rule is
					// the same: append our observation.
					let new_value = match req.headers().get(&name).and_then(|v| v.to_str().ok()) {
						Some(existing) if !existing.is_empty() => format!("{existing}, {client_ip}"),
						_ => client_ip.clone(),
					};
					let value =
						http::HeaderValue::from_str(&new_value).expect("ip-derived header value is ASCII");
					req.headers_mut().insert(name, value);
				}
				HeaderAction::XRealIpOverwrite => {
					let name = http::header::HeaderName::from_static("x-real-ip");
					let value =
						http::HeaderValue::from_str(&client_ip).expect("ip-derived header value is ASCII");
					req.headers_mut().insert(name, value);
				}
				HeaderAction::ForwardedAppend => {
					let name = http::header::HeaderName::from_static("forwarded");
					let new_token = render_forwarded_pair(peer, local, proto);
					let new_value = match req.headers().get(&name).and_then(|v| v.to_str().ok()) {
						Some(existing) if !existing.is_empty() => format!("{existing}, {new_token}"),
						_ => new_token,
					};
					let value = http::HeaderValue::from_str(&new_value).expect("Forwarded value is ASCII");
					req.headers_mut().insert(name, value);
				}
			}
		}
		Ok(Decision::Continue)
	}
}

/// Render an RFC 7239 `Forwarded:` pair for one hop. IPv4 / IPv6
/// addresses follow §6: IPv4 uses dotted-decimal; IPv6 uses
/// `"[…]"` quoted brackets. `proto` is `https` if TLS is in play,
/// otherwise `http`.
fn render_forwarded_pair(peer: IpAddr, local: IpAddr, proto: &str) -> String {
	format!("for={};by={};proto={}", forwarded_node(peer), forwarded_node(local), proto)
}

/// Render a single `for=` / `by=` node value. IPv6 needs RFC 7239
/// §4-mandated quoted-bracket form (`"[2001:db8::1]"`); IPv4 prints
/// raw. The brackets distinguish the address from the optional
/// `:port` suffix per ABNF, and the quotes carry the brackets past
/// the `token` grammar so the value stays parseable.
fn forwarded_node(ip: IpAddr) -> String {
	match ip {
		IpAddr::V4(_) => ip.to_string(),
		IpAddr::V6(_) => format!("\"[{ip}]\""),
	}
}

/// Args parser exposed as a registry-friendly factory.
///
/// Args shape (all fields optional):
///
/// ```json
/// {
///   "headers": ["x-forwarded-for", "x-real-ip", "forwarded"],
///   "trusted_proxies": ["10.0.0.0/8", "192.168.0.0/16", "fd00::/8"],
///   "strip_inbound_forwarded": true
/// }
/// ```
///
/// - `headers` defaults to `["x-forwarded-for", "x-real-ip"]`. The
///   recognised names are `x-forwarded-for`, `x-real-ip`,
///   `forwarded`; anything else is rejected.
/// - `trusted_proxies` defaults to `[]` — the most conservative
///   posture. Operators behind a known reverse-proxy chain (LAN
///   load balancer, CDN egress) should populate this list with the
///   CIDR ranges those proxies dial from. The `reverse_proxy`
///   preset substitutes the RFC1918 + ULA ranges into this slot.
/// - `strip_inbound_forwarded` defaults to `true` — removes inbound
///   `X-Forwarded-Proto` / `X-Forwarded-Host` (which the middleware
///   does not honour) so the upstream sees only the headers
///   `forward_client_ip` itself emits.
///
/// # Errors
/// Returns [`FactoryError`] when `headers` / `trusted_proxies` /
/// `strip_inbound_forwarded` are shape-mismatched or contain
/// unparseable members.
pub fn factory(args: &serde_json::Value) -> Result<MiddlewareInst, FactoryError> {
	let actions = parse_headers(args.get("headers"))?;
	let trusted_proxies = parse_trusted_proxies(args.get("trusted_proxies"))?;
	let strip_inbound_forwarded = parse_strip_inbound_forwarded(args.get("strip_inbound_forwarded"))?;
	Ok(MiddlewareInst::L7Request(Arc::new(ForwardClientIp {
		actions,
		trusted_proxies,
		strip_inbound_forwarded,
	})))
}

fn parse_headers(value: Option<&serde_json::Value>) -> Result<Vec<HeaderAction>, FactoryError> {
	let Some(value) = value else {
		return Ok(vec![HeaderAction::XForwardedForAppend, HeaderAction::XRealIpOverwrite]);
	};
	let arr = value
		.as_array()
		.ok_or_else(|| FactoryError::Invalid("args.headers must be an array".to_string()))?;
	let mut out = Vec::with_capacity(arr.len());
	for item in arr {
		let s = item
			.as_str()
			.ok_or_else(|| FactoryError::Invalid("args.headers items must be strings".to_string()))?;
		let lower = s.to_ascii_lowercase();
		let action = match lower.as_str() {
			"x-forwarded-for" => HeaderAction::XForwardedForAppend,
			"x-real-ip" => HeaderAction::XRealIpOverwrite,
			"forwarded" => HeaderAction::ForwardedAppend,
			_ => {
				return Err(FactoryError::Invalid(format!(
					"unsupported header {s:?}; supported: x-forwarded-for, x-real-ip, forwarded",
				)));
			}
		};
		out.push(action);
	}
	Ok(out)
}

fn parse_trusted_proxies(value: Option<&serde_json::Value>) -> Result<Vec<IpNet>, FactoryError> {
	let Some(value) = value else {
		return Ok(Vec::new());
	};
	let arr = value
		.as_array()
		.ok_or_else(|| FactoryError::Invalid("args.trusted_proxies must be an array".to_string()))?;
	let mut out = Vec::with_capacity(arr.len());
	for item in arr {
		let s = item.as_str().ok_or_else(|| {
			FactoryError::Invalid("args.trusted_proxies items must be CIDR strings".to_string())
		})?;
		let net: IpNet =
			s.parse().map_err(|e| FactoryError::Invalid(format!("invalid CIDR {s:?}: {e}")))?;
		out.push(net);
	}
	Ok(out)
}

fn parse_strip_inbound_forwarded(value: Option<&serde_json::Value>) -> Result<bool, FactoryError> {
	let Some(value) = value else {
		return Ok(true);
	};
	value.as_bool().ok_or_else(|| {
		FactoryError::Invalid("args.strip_inbound_forwarded must be a boolean".to_string())
	})
}

/// Plug `forward_client_ip` into a `MiddlewareFactories` registry.
pub fn register(factories: &mut MiddlewareFactories) {
	factories.register("forward_client_ip", MiddlewareKind::L7Request, factory);
}

#[cfg(test)]
mod tests {
	use std::net::IpAddr;

	use super::*;

	#[test]
	fn forwarded_node_quotes_ipv6_brackets() {
		let v6: IpAddr = "2001:db8::1".parse().unwrap();
		assert_eq!(forwarded_node(v6), "\"[2001:db8::1]\"");
		let v4: IpAddr = "203.0.113.5".parse().unwrap();
		assert_eq!(forwarded_node(v4), "203.0.113.5");
	}

	#[test]
	fn render_forwarded_pair_round_trips() {
		let peer: IpAddr = "203.0.113.5".parse().unwrap();
		let local: IpAddr = "10.0.0.1".parse().unwrap();
		assert_eq!(
			render_forwarded_pair(peer, local, "https"),
			"for=203.0.113.5;by=10.0.0.1;proto=https",
		);
	}

	#[test]
	fn parse_trusted_proxies_accepts_v4_and_v6_cidrs() {
		let v = serde_json::json!(["10.0.0.0/8", "fd00::/8"]);
		let nets = parse_trusted_proxies(Some(&v)).expect("parse ok");
		assert_eq!(nets.len(), 2);
		assert!(nets[0].contains(&"10.1.2.3".parse::<IpAddr>().unwrap()));
		assert!(nets[1].contains(&"fd11:2233::1".parse::<IpAddr>().unwrap()));
	}

	#[test]
	fn parse_trusted_proxies_rejects_garbage() {
		let v = serde_json::json!(["not a cidr"]);
		let err = parse_trusted_proxies(Some(&v)).expect_err("must reject");
		let msg = err.message();
		assert!(msg.contains("invalid CIDR"), "{msg}");
	}
}
