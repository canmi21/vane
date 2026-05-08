//! In-process DNS server + [`vane_engine::acme::DnsProvider`] impl
//! for ACME DNS-01 tests.
//!
//! Spawns a `hickory-server` `Server` on an ephemeral UDP port; the
//! provider's `set_txt` / `delete_txt` calls mutate a zone store
//! the server reads from on every query. `wait_propagated` runs
//! real DNS queries against the same server so the resolver path is
//! exercised, not just the in-memory store.
//!
//! Pure Rust, no Docker — runs anywhere a tokio runtime can spawn
//! a UDP listener. Tests that need both an ACME server and a DNS
//! authority (Pebble + DNS-01 e2e) point Pebble's `-dnsserver` at
//! [`MockDns::addr`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hickory_proto::op::{Header, HeaderCounts, MessageType, Metadata, ResponseCode};
use hickory_proto::rr::rdata::TXT;
use hickory_proto::rr::{RData, Record, RecordType};
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo, Server};
use hickory_server::zone_handler::MessageResponseBuilder;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::net::{TcpListener, UdpSocket};
use vane_engine::acme::{DnsProvider, DnsProviderError};

/// In-process mock DNS authority. Drop the value to stop the
/// server.
///
/// `addr()` returns the `127.0.0.1:<ephemeral_port>` the server
/// is listening on; pass it to consumers (e.g. Pebble's
/// `-dnsserver` flag) so they query the mock instead of the
/// public Internet.
pub struct MockDns {
	addr: SocketAddr,
	zone_store: Arc<Mutex<ZoneStore>>,
	_server: Server<MockDnsHandler>,
}

#[derive(Debug, Error)]
pub enum MockDnsError {
	#[error("failed to bind ephemeral udp port: {0}")]
	Bind(#[from] std::io::Error),
}

impl MockDns {
	/// Bind a UDP socket on `0.0.0.0:0`, register a hickory
	/// `Server` against an empty zone store, and return the
	/// fixture handle.
	///
	/// Binds on the unspecified address so that integration
	/// fixtures running outside the test process (e.g. Pebble in
	/// a Docker container reaching the host via
	/// `host.docker.internal`) can route to the server. Local
	/// in-process queries still hit `127.0.0.1` via [`Self::addr`].
	///
	/// # Errors
	///
	/// `MockDnsError::Bind` when the kernel refuses the ephemeral
	/// bind (effectively never on a healthy system).
	pub async fn start() -> Result<Self, MockDnsError> {
		// Bind UDP first to pin an ephemeral port, then bind a matching
		// TCP listener on the same port — Pebble's miekg/dns (when
		// configured with `-dnsserver host:port`) may fall back to TCP
		// on UDP truncation, so serving both keeps the fixture
		// resolver-agnostic.
		//
		// UDP and TCP have independent port spaces, so the kernel can
		// hand the same ephemeral port to a parallel test's TCP socket
		// between our UDP `bind` and TCP `bind`. When that race trips,
		// retry with a fresh ephemeral pick. `MAX_BIND_ATTEMPTS` keeps
		// the loop bounded under unusual port pressure.
		const MAX_BIND_ATTEMPTS: u32 = 16;
		let (udp, tcp) = {
			let mut last_err: Option<std::io::Error> = None;
			let mut bound = None;
			for _ in 0..MAX_BIND_ATTEMPTS {
				let udp = UdpSocket::bind("0.0.0.0:0").await?;
				let port = udp.local_addr()?.port();
				match TcpListener::bind(("0.0.0.0", port)).await {
					Ok(tcp) => {
						bound = Some((udp, tcp));
						break;
					}
					Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
						last_err = Some(e);
						drop(udp);
					}
					Err(e) => return Err(MockDnsError::Bind(e)),
				}
			}
			bound.ok_or_else(|| {
				MockDnsError::Bind(last_err.unwrap_or_else(|| {
					std::io::Error::new(std::io::ErrorKind::AddrInUse, "exhausted bind attempts")
				}))
			})?
		};
		let port = udp.local_addr()?.port();
		// Surface a connect-friendly address — `0.0.0.0:<port>`
		// from `local_addr` is a wildcard bind and not a valid
		// destination.
		let addr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
		let zone_store = Arc::new(Mutex::new(ZoneStore::default()));
		let handler = MockDnsHandler { zone_store: Arc::clone(&zone_store) };
		let mut server = Server::new(handler);
		server.register_socket(udp);
		// 5 s TCP read timeout / 64 KiB response buffer — generous
		// enough for any TXT response we might emit.
		server.register_listener(tcp, Duration::from_secs(5), 64 * 1024);
		Ok(Self { addr, zone_store, _server: server })
	}

	/// Connect-friendly address (`127.0.0.1:<port>`). Pass this
	/// to local resolvers; remote consumers (Pebble) need
	/// `host.docker.internal:<port>` — see
	/// [`super::Pebble::start_with_dns_resolver`].
	#[must_use]
	pub fn addr(&self) -> SocketAddr {
		self.addr
	}

	/// Build a [`DnsProvider`] impl that writes into this mock's
	/// zone store. Cheap clone — the provider holds an `Arc` of
	/// the same store the server reads.
	#[must_use]
	pub fn provider(&self) -> Arc<dyn DnsProvider> {
		Arc::new(MockDnsProvider { server_addr: self.addr, zone_store: Arc::clone(&self.zone_store) })
	}

	/// Test-only: snapshot the current TXT records for `name`. The
	/// returned vector is empty when the name has no records.
	#[must_use]
	pub fn txt_records(&self, name: &str) -> Vec<String> {
		self.zone_store.lock().txt.get(&normalise_name(name)).cloned().unwrap_or_default()
	}
}

#[derive(Default, Debug)]
struct ZoneStore {
	/// FQDN (lowercased, no trailing dot) → list of TXT values.
	txt: HashMap<String, Vec<String>>,
}

#[derive(Debug)]
struct MockDnsProvider {
	server_addr: SocketAddr,
	zone_store: Arc<Mutex<ZoneStore>>,
}

#[async_trait]
impl DnsProvider for MockDnsProvider {
	async fn set_txt(&self, name: &str, value: &str) -> Result<(), DnsProviderError> {
		let key = normalise_name(name);
		self.zone_store.lock().txt.entry(key).or_default().push(value.to_owned());
		Ok(())
	}

	async fn delete_txt(&self, name: &str) -> Result<(), DnsProviderError> {
		self.zone_store.lock().txt.remove(&normalise_name(name));
		Ok(())
	}

	async fn wait_propagated(
		&self,
		name: &str,
		value: &str,
		timeout: Duration,
	) -> Result<(), DnsProviderError> {
		// Mock writes are immediately visible in the zone store, but
		// running an actual DNS query against `self.server_addr`
		// exercises the hickory wiring — that's the whole point of
		// using a real server fixture rather than a stub provider.
		let resolver = build_resolver_for_addr(self.server_addr);
		let deadline = Instant::now() + timeout;
		loop {
			if let Ok(lookup) = resolver.txt_lookup(name).await
				&& lookup_contains_txt(&lookup, value.as_bytes())
			{
				return Ok(());
			}
			if Instant::now() >= deadline {
				return Err(DnsProviderError::PropagationTimeout(name.to_owned()));
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	}
}

fn build_resolver_for_addr(addr: SocketAddr) -> TokioResolver {
	let mut conn = ConnectionConfig::udp();
	conn.port = addr.port();
	let cfg = ResolverConfig::from_parts(
		None,
		vec![],
		vec![NameServerConfig::new(addr.ip(), true, vec![conn])],
	);
	let mut opts = ResolverOpts::default();
	opts.cache_size = 0;
	opts.attempts = 1;
	opts.timeout = Duration::from_millis(200);
	TokioResolver::builder_with_config(cfg, TokioRuntimeProvider::default())
		.with_options(opts)
		.build()
		.expect("resolver builder")
}

fn lookup_contains_txt(lookup: &hickory_resolver::lookup::Lookup, expected: &[u8]) -> bool {
	lookup.answers().iter().any(|record| {
		if let RData::TXT(txt) = &record.data {
			txt.txt_data.iter().any(|d| d.as_ref() == expected)
		} else {
			false
		}
	})
}

fn normalise_name(name: &str) -> String {
	let mut s = name.to_ascii_lowercase();
	if s.ends_with('.') {
		s.pop();
	}
	s
}

/// hickory `RequestHandler` that consults the shared `ZoneStore`
/// for TXT queries. Other query types return `NotImplemented`
/// because Pebble's validator only ever asks for TXT.
struct MockDnsHandler {
	zone_store: Arc<Mutex<ZoneStore>>,
}

#[async_trait]
impl RequestHandler for MockDnsHandler {
	async fn handle_request<R: ResponseHandler, T: Time>(
		&self,
		request: &Request,
		mut response_handle: R,
	) -> ResponseInfo {
		let req_metadata = request.metadata;
		let queries = request.queries.queries();
		let Some(query) = queries.iter().next() else {
			return reply_error(&mut response_handle, request, ResponseCode::FormErr).await;
		};

		if query.query_type() != RecordType::TXT {
			return reply_error(&mut response_handle, request, ResponseCode::NotImp).await;
		}

		let name_str = normalise_name(&query.name().to_string());
		let values: Vec<String> =
			self.zone_store.lock().txt.get(&name_str).cloned().unwrap_or_default();

		// Build owned `Record`s the iterator can hand back to the
		// response builder. `RData::TXT` owns its strings, so the
		// `Vec<Record>` outlives the iterator hickory needs.
		let answers: Vec<Record> = values
			.into_iter()
			.map(|v| {
				let txt = TXT::new(vec![v]);
				let name = query.name().clone().into();
				Record::from_rdata(name, 60, RData::TXT(txt))
			})
			.collect();

		let mut response_metadata = Metadata::response_from_request(&req_metadata);
		response_metadata.message_type = MessageType::Response;
		response_metadata.response_code = ResponseCode::NoError;
		response_metadata.authoritative = true;
		let builder = MessageResponseBuilder::from_message_request(request);
		let message = builder.build(response_metadata, answers.iter(), [].iter(), [].iter(), [].iter());
		response_handle.send_response(message).await.unwrap_or_else(|_| {
			// On wire-level send failure synthesise a ResponseInfo so
			// the trait contract is satisfied; the test will time out
			// downstream and surface the real cause.
			ResponseInfo::from(synth_header(req_metadata, ResponseCode::ServFail))
		})
	}
}

async fn reply_error<R: ResponseHandler>(
	response_handle: &mut R,
	request: &Request,
	code: ResponseCode,
) -> ResponseInfo {
	let req_metadata = request.metadata;
	let mut metadata = Metadata::response_from_request(&req_metadata);
	metadata.response_code = code;
	let builder = MessageResponseBuilder::from_message_request(request);
	let message = builder.build(metadata, [].iter(), [].iter(), [].iter(), [].iter());
	response_handle
		.send_response(message)
		.await
		.unwrap_or_else(|_| ResponseInfo::from(synth_header(req_metadata, code)))
}

fn synth_header(req_metadata: Metadata, code: ResponseCode) -> Header {
	let mut metadata = Metadata::response_from_request(&req_metadata);
	metadata.response_code = code;
	Header { metadata, counts: HeaderCounts::default() }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn set_txt_then_query_returns_value() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		provider.set_txt("_acme-challenge.example.test", "value-A").await.expect("set");

		let resolver = build_resolver_for_addr(mock.addr());
		let lookup = resolver.txt_lookup("_acme-challenge.example.test").await.expect("lookup");
		assert!(lookup_contains_txt(&lookup, b"value-A"), "value-A must be present in the answer");
	}

	#[tokio::test]
	async fn delete_txt_removes_record() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		provider.set_txt("_acme-challenge.example.test", "v").await.expect("set");
		provider.delete_txt("_acme-challenge.example.test").await.expect("delete");
		assert!(mock.txt_records("_acme-challenge.example.test").is_empty());
	}

	#[tokio::test]
	async fn delete_txt_idempotent_when_absent() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		// No prior set; delete must not error.
		provider.delete_txt("_acme-challenge.never-set.test").await.expect("idempotent delete");
	}

	#[tokio::test]
	async fn wait_propagated_returns_ok_for_present_record() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		provider.set_txt("_acme-challenge.api.test", "ka-XYZ").await.expect("set");
		provider
			.wait_propagated("_acme-challenge.api.test", "ka-XYZ", Duration::from_secs(2))
			.await
			.expect("propagated");
	}

	#[tokio::test]
	async fn wait_propagated_times_out_for_absent_record() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		match provider
			.wait_propagated("_acme-challenge.missing.test", "ka", Duration::from_millis(150))
			.await
		{
			Err(DnsProviderError::PropagationTimeout(name)) => {
				assert!(name.contains("missing"), "got {name}");
			}
			other => panic!("expected PropagationTimeout, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn wait_propagated_observes_late_set_txt() {
		// Spawn wait_propagated with a generous timeout; concurrently
		// run set_txt after a small delay. The wait must succeed once
		// the record lands. Verifies the polling loop actually
		// re-queries instead of caching the initial NXDOMAIN.
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		let provider_clone = Arc::clone(&provider);
		let writer = tokio::spawn(async move {
			tokio::time::sleep(Duration::from_millis(80)).await;
			provider_clone.set_txt("_acme-challenge.late.test", "ka-LATE").await.expect("late set");
		});
		provider
			.wait_propagated("_acme-challenge.late.test", "ka-LATE", Duration::from_secs(2))
			.await
			.expect("propagated after late write");
		writer.await.expect("writer joined");
	}

	#[tokio::test]
	async fn name_normalisation_is_case_insensitive_and_dot_tolerant() {
		let mock = MockDns::start().await.expect("start");
		let provider = mock.provider();
		provider.set_txt("_ACME-Challenge.Example.Test.", "v").await.expect("set");
		assert_eq!(mock.txt_records("_acme-challenge.example.test"), vec!["v".to_owned()]);
	}
}
