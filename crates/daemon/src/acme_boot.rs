//! Daemon-side boot wiring for ACME-managed certificates.
//!
//! Three responsibilities:
//!
//! 1. Open `FsAcmeStore` + `ManagedCertRegistry` if the compiled
//!    config declares any `tls.managed` cert. The store path comes
//!    from the `VANE_ACME_DIR` env var (default
//!    `/var/lib/vaned/acme/` per `spec/crates/engine-acme.md` § _Storage layout_).
//! 2. After `FlowGraph::link` succeeds, kick off background
//!    issuance tasks for every declared SNI that doesn't already
//!    have a cached cert. Each task surfaces failures via
//!    `tracing::error!`; the daemon doesn't abort on issuance
//!    failure so other functionality continues.
//! 3. Auto-bind a synthetic plaintext `:80` listener whose only
//!    job is serving HTTP-01 challenges, when the operator's
//!    config has no `:80` listener. Per `spec/crates/engine-acme.md` § _Challenge: HTTP-01_,
//!    bind failures (`EACCES` on a privileged port without
//!    `CAP_NET_BIND_SERVICE`, `EADDRINUSE`) log at ERROR but don't
//!    abort boot.
//!
//! Feature-gated behind `acme`. Non-ACME builds compile this
//! module out entirely so the daemon binary doesn't pull
//! `instant-acme` / `rcgen` / `fs4` / `futures`.

#![cfg(feature = "acme")]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use vane_core::ir::SymbolicFlowGraph;
use vane_core::rule::{ChallengeKind, ManagedSpec};
use vane_engine::acme::{FsAcmeStore, ManagedCertRegistry, RegistryError, RenewalJob};
use vane_engine::flow_graph::FlowGraph;

/// Default storage root per `spec/crates/engine-acme.md` § _Storage layout_. Overridden by `VANE_ACME_DIR`.
const DEFAULT_ACME_DIR: &str = "/var/lib/vaned/acme";

/// `Some(registry)` when the compiled config declares at least one
/// `tls.managed` cert, `None` otherwise. Construction failure on a
/// config that does want managed certs surfaces as an error so the
/// daemon doesn't silently boot in a state where managed listeners
/// can never get a cert.
pub(crate) async fn open_registry_if_needed(
	symbolic: &SymbolicFlowGraph,
) -> Result<Option<Arc<ManagedCertRegistry>>, Box<dyn std::error::Error + Send + Sync>> {
	if !any_managed_cert(symbolic) {
		return Ok(None);
	}
	let dir = acme_dir_from_env();
	std::fs::create_dir_all(&dir)
		.map_err(|e| format!("create acme storage root {}: {e}", dir.display()))?;
	let store = FsAcmeStore::open(&dir).map_err(|e| format!("open acme store: {e}"))?;
	let registry = ManagedCertRegistry::open(Arc::new(store))
		.await
		.map_err(|e| format!("open acme registry: {e}"))?;
	info!(
		target: "vane::acme",
		acme_dir = %dir.display(),
		"ACME registry opened",
	);
	Ok(Some(registry))
}

fn acme_dir_from_env() -> PathBuf {
	std::env::var("VANE_ACME_DIR").map_or_else(|_| PathBuf::from(DEFAULT_ACME_DIR), PathBuf::from)
}

fn any_managed_cert(symbolic: &SymbolicFlowGraph) -> bool {
	symbolic.meta.listener_tls.values().any(|spec| !spec.managed_snis.is_empty())
}

/// Walk the linked `FlowGraph`'s listener TLS specs, declare every
/// managed SNI to the registry, and kick off background issuance
/// for SNIs lacking a cached cert. Returns the spawned handles so
/// the caller can join on shutdown if desired.
pub(crate) fn kick_off_managed_issuance(
	registry: &Arc<ManagedCertRegistry>,
	graph: &Arc<FlowGraph>,
	cancel: &CancellationToken,
) -> Vec<tokio::task::JoinHandle<()>> {
	// One issuance task per (sni, directory_url, contact) tuple.
	// We collect the unique tuples by walking listener_tls.
	let plans = collect_issuance_plans(graph);
	let snis: Vec<String> = plans.iter().map(|p| p.sni.clone()).collect();
	let needs_issue = registry.declare_managed(&snis);
	let needs_set: BTreeSet<String> = needs_issue.into_iter().collect();

	// Register renewal jobs for *every* declared SNI — including ones
	// already covered by hydrated certs — so the scheduler can pick
	// up renewal triggers (`now + renew_before >= not_after`, ARI,
	// `force_renew`) without re-walking the listener spec at tick
	// time. Jobs are keyed by SNI + replaced on reload, so a
	// challenge-kind switch (http-01 → dns-01) takes effect at the
	// next tick without a daemon restart.
	for plan in &plans {
		match build_renewal_job(plan) {
			Ok(job) => registry.register_renewal_job(&plan.sni, job),
			Err(e) => {
				error!(
					target: "vane::acme",
					sni = %plan.sni,
					error = %e,
					"renewal job build failed; scheduler will not retry this SNI",
				);
			}
		}
	}

	let mut handles = Vec::new();
	for plan in plans {
		if !needs_set.contains(&plan.sni) {
			continue;
		}
		let registry = Arc::clone(registry);
		let cancel = cancel.clone();
		handles.push(tokio::spawn(async move {
			run_one_issuance(registry, plan, cancel).await;
		}));
	}
	handles
}

/// Translate an [`IssuancePlan`] into a [`RenewalJob`] the registry
/// can use at scheduler-tick time. Builds the DNS provider once
/// here (per `spec/crates/engine-acme.md` § _Challenge: DNS-01_) so the scheduler doesn't
/// have to re-parse the JSON config at every tick.
fn build_renewal_job(plan: &IssuancePlan) -> Result<RenewalJob, String> {
	let renew_before = plan.renew_before;
	let dns = match plan.challenge {
		ChallengeKind::Http01 => None,
		ChallengeKind::Dns01 => Some(build_dns_provider(plan.dns_provider.as_ref())?),
	};
	Ok(RenewalJob {
		directory_url: plan.directory_url.clone(),
		contact: plan.contact.clone(),
		challenge: plan.challenge,
		dns,
		renew_before,
		extra_root_ca_pem: None,
	})
}

#[derive(Clone, Debug)]
struct IssuancePlan {
	sni: String,
	directory_url: String,
	contact: Vec<String>,
	challenge: ChallengeKind,
	/// `Some` when `challenge == Dns01` — the operator-supplied
	/// `dns_provider` JSON object, kept opaque here and parsed
	/// inside [`run_one_issuance`] so the boot path doesn't have
	/// to know about every provider kind.
	dns_provider: Option<serde_json::Value>,
	/// Pre-parsed `renew_before` from the rule's
	/// [`ManagedSpec::renew_before`]. Carried into the
	/// [`RenewalJob`] so the scheduler doesn't have to re-parse the
	/// duration literal at every tick.
	renew_before: std::time::Duration,
}

fn collect_issuance_plans(graph: &FlowGraph) -> Vec<IssuancePlan> {
	let symbolic = graph.symbolic();
	let mut by_sni: BTreeMap<String, IssuancePlan> = BTreeMap::new();
	for spec in symbolic.meta.listener_tls.values() {
		for (sni, managed) in &spec.managed_snis {
			by_sni.entry(sni.clone()).or_insert_with(|| IssuancePlan {
				sni: sni.clone(),
				directory_url: managed.directory_url.clone(),
				contact: managed.contact.clone(),
				challenge: managed.challenge,
				dns_provider: managed.dns_provider.clone(),
				// `ManagedSpec::renew_before_duration` returns a
				// post-validate `Duration`; the upstream `compile`
				// pass guarantees it parses cleanly, so a parse error
				// here would be an engine bug. We surface it as a
				// best-effort 30d fallback rather than crashing the
				// daemon — the worst case is a renewal cadence
				// mismatch the operator can fix at next reload.
				renew_before: validated_renew_before(managed),
			});
		}
	}
	by_sni.into_values().collect()
}

fn validated_renew_before(managed: &ManagedSpec) -> std::time::Duration {
	managed.renew_before_duration().unwrap_or_else(|e| {
		warn!(
			target: "vane::acme",
			renew_before = %managed.renew_before,
			error = %e,
			"managed cert renew_before failed to re-parse; defaulting to 30d",
		);
		std::time::Duration::from_secs(30 * 24 * 60 * 60)
	})
}

async fn run_one_issuance(
	registry: Arc<ManagedCertRegistry>,
	plan: IssuancePlan,
	cancel: CancellationToken,
) {
	let result = match plan.challenge {
		ChallengeKind::Http01 => {
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					info!(target: "vane::acme", sni = %plan.sni, "issuance cancelled by shutdown");
					return;
				}
				r = registry.issue_http01(&plan.sni, &plan.directory_url, &plan.contact) => r,
			}
		}
		ChallengeKind::Dns01 => {
			let dns = match build_dns_provider(plan.dns_provider.as_ref()) {
				Ok(d) => d,
				Err(e) => {
					error!(
						target: "vane::acme",
						sni = %plan.sni,
						error = %e,
						"dns provider config invalid; dns-01 issuance skipped",
					);
					return;
				}
			};
			tokio::select! {
				biased;
				() = cancel.cancelled() => {
					info!(target: "vane::acme", sni = %plan.sni, "issuance cancelled by shutdown");
					return;
				}
				r = registry.issue_dns01(&plan.sni, &plan.directory_url, &plan.contact, dns) => r,
			}
		}
	};
	match result {
		Ok(_) => {
			info!(target: "vane::acme", sni = %plan.sni, "managed cert issued");
		}
		Err(RegistryError::RateLimited { retry_after }) => {
			warn!(
				target: "vane::acme",
				sni = %plan.sni,
				retry_after_secs = retry_after.map(|d| d.as_secs()),
				"managed cert issuance rate-limited by CA",
			);
		}
		Err(e) => {
			error!(
				target: "vane::acme",
				sni = %plan.sni,
				error = %e,
				"managed cert issuance failed",
			);
		}
	}
}

/// Translate the operator's `dns_provider` JSON object into a
/// concrete `Arc<dyn DnsProvider>`. Each provider kind has its
/// own `kind` discriminator per `spec/crates/engine-acme.md` § _Cloudflare provider_. Unknown kinds and missing config are
/// boot-time-fatal for the affected SNI (we surface them via
/// the calling `run_one_issuance` log).
fn build_dns_provider(
	raw: Option<&serde_json::Value>,
) -> Result<Arc<dyn vane_engine::acme::DnsProvider>, String> {
	let value = raw.ok_or_else(|| "dns_provider missing for dns-01 challenge".to_owned())?;
	let kind = value
		.get("kind")
		.and_then(serde_json::Value::as_str)
		.ok_or_else(|| "dns_provider.kind missing or non-string".to_owned())?;
	match kind {
		#[cfg(feature = "cloudflare")]
		"cloudflare" => {
			let cfg: vane_engine::acme::dns::CloudflareConfig =
				serde_json::from_value(value.clone()).map_err(|e| format!("dns_provider parse: {e}"))?;
			let provider = vane_engine::acme::dns::CloudflareDnsProvider::from_config(&cfg)
				.map_err(|e| format!("cloudflare provider: {e}"))?;
			Ok(Arc::new(provider))
		}
		other => Err(format!("dns_provider kind {other:?} not supported in this build")),
	}
}

/// Bind a synthetic plaintext `:80` listener whose only role is
/// serving HTTP-01 challenges — only when the operator's config
/// has no `:80` listener AND at least one tls.managed declares
/// http-01.
///
/// Per `spec/crates/engine-acme.md` § _Challenge: HTTP-01_:
/// - Successful bind: `WARN`-level log; the listener serves the
///   challenge route and 404s everything else.
/// - Bind failure: `ERROR`-level log; the daemon continues
///   serving traffic. Affected ACME issuances will fail HTTP-01
///   validation.
///
/// Returns the spawned task handles (one per dual-stack address)
/// or `None` when auto-bind isn't called for.
pub(crate) async fn maybe_auto_bind_port_80(
	registry: Arc<ManagedCertRegistry>,
	graph: &FlowGraph,
	cancel: &CancellationToken,
) -> Vec<tokio::task::JoinHandle<()>> {
	if !needs_auto_bind(graph) {
		return Vec::new();
	}
	// `spec/crates/engine-acme.md` § _Challenge: HTTP-01_: dual-stack `0.0.0.0:80` + `[::]:80`.
	// Each task binds independently; one failing doesn't block the other.
	let addrs = [
		SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80),
		SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 80),
	];
	let mut handles = Vec::new();
	for addr in addrs {
		let registry = Arc::clone(&registry);
		let cancel = cancel.clone();
		handles.push(tokio::spawn(async move {
			run_auto_bind(addr, registry, cancel).await;
		}));
	}
	handles
}

fn needs_auto_bind(graph: &FlowGraph) -> bool {
	let symbolic = graph.symbolic();
	let any_http01 = symbolic
		.meta
		.listener_tls
		.values()
		.any(|spec| spec.managed_snis.values().any(|m| matches!(m.challenge, ChallengeKind::Http01)));
	if !any_http01 {
		return false;
	}
	// True when no listener address is on port 80. (A TLS-on-:80
	// listener is treated as "no plaintext :80" per
	// `spec/crates/engine-acme.md` § _Challenge: HTTP-01_; auto-bind will
	// attempt and fail with EADDRINUSE, which is the documented
	// behaviour.)
	!symbolic.meta.listener_kinds.keys().any(|addr| addr.port() == 80)
}

async fn run_auto_bind(
	addr: SocketAddr,
	registry: Arc<ManagedCertRegistry>,
	cancel: CancellationToken,
) {
	let listener = match TcpListener::bind(addr).await {
		Ok(l) => l,
		Err(e) => {
			error!(
				target: "vane::acme",
				addr = %addr,
				error = %e,
				"acme: auto-bind :80 plaintext listener failed; HTTP-01 validation will fail until \
				 the operator configures an explicit :80 listener or grants CAP_NET_BIND_SERVICE",
			);
			return;
		}
	};
	warn!(
		target: "vane::acme",
		addr = %addr,
		"acme: auto-bound :80 plaintext listener for HTTP-01 challenges; \
		 configure an explicit :80 listener to suppress this notice",
	);

	loop {
		tokio::select! {
			biased;
			() = cancel.cancelled() => return,
			accept = listener.accept() => {
				let (stream, _peer) = match accept {
					Ok(p) => p,
					Err(e) => {
						warn!(
							target: "vane::acme",
							addr = %addr,
							error = %e,
							"acme auto-bind accept failed; continuing",
						);
						continue;
					}
				};
				let registry = Arc::clone(&registry);
				let cancel = cancel.clone();
				tokio::spawn(async move {
					serve_one_connection(stream, registry, cancel).await;
				});
			}
		}
	}
}

async fn serve_one_connection(
	stream: tokio::net::TcpStream,
	registry: Arc<ManagedCertRegistry>,
	cancel: CancellationToken,
) {
	let io = TokioIo::new(stream);
	let svc = service_fn(move |req: Request<Incoming>| {
		let registry = Arc::clone(&registry);
		async move { Ok::<_, std::convert::Infallible>(handle_one_request(&registry, &req)) }
	});
	let conn = http1::Builder::new().serve_connection(io, svc);
	tokio::select! {
		biased;
		() = cancel.cancelled() => {}
		res = conn => {
			if let Err(e) = res {
				// `hyper::Error` from a closed connection is normal; only
				// surface unexpected shapes.
				tracing::trace!(target: "vane::acme", error = %e, "auto-bind conn ended");
			}
		}
	}
}

const ACME_PATH_PREFIX: &str = "/.well-known/acme-challenge/";

fn handle_one_request(
	registry: &ManagedCertRegistry,
	req: &Request<Incoming>,
) -> Response<Full<Bytes>> {
	let path = req.uri().path();
	if !path.starts_with(ACME_PATH_PREFIX) {
		return Response::builder()
			.status(StatusCode::NOT_FOUND)
			.header(hyper::header::CONTENT_TYPE, "text/plain")
			.body(Full::from(Bytes::from_static(b"acme auto-bind: not the challenge path")))
			.expect("static response");
	}
	let token = path.strip_prefix(ACME_PATH_PREFIX).unwrap_or("");
	let host = req
		.headers()
		.get(hyper::header::HOST)
		.and_then(|v| v.to_str().ok())
		.map(host_strip_port)
		.unwrap_or_default();
	match registry.lookup_http01(&host, token) {
		Some(key_authorization) => Response::builder()
			.status(StatusCode::OK)
			.header(hyper::header::CONTENT_TYPE, "application/octet-stream")
			.body(Full::from(Bytes::from(key_authorization)))
			.expect("static response"),
		None => Response::builder()
			.status(StatusCode::NOT_FOUND)
			.header(hyper::header::CONTENT_TYPE, "text/plain")
			.body(Full::from(Bytes::from_static(b"acme challenge not found")))
			.expect("static response"),
	}
}

fn host_strip_port(raw: &str) -> String {
	raw.split(':').next().unwrap_or("").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn host_strip_port_normalises_case_and_drops_port() {
		assert_eq!(host_strip_port("API.example.COM:8080"), "api.example.com");
		assert_eq!(host_strip_port("api.example.com"), "api.example.com");
	}

	// `acme_dir_from_env` is exercised through the boot path's
	// integration tests rather than a unit test — touching the
	// process-wide env in a parallel-cargo test environment is
	// inherently racy and Rust 2024 marks `env::remove_var` unsafe
	// (workspace lint `unsafe_code = deny` blocks the call here).
}
