use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::net::SocketAddr;
use std::ops::Index;
use std::sync::Arc;

use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, L4BytesMiddleware, L4Fetch, L4PeekMiddleware, L7Fetch,
	L7RequestMiddleware, L7ResponseMiddleware, MiddlewareId, MiddlewareKind, Node, NodeId,
	SymbolicFlowGraph,
	rule::{ListenerTlsSpec, TlsConfig},
};

use crate::factories::{
	FactoryError, FetchFactories, FetchFactoryEntry, MiddlewareFactories, MiddlewareFactoryEntry,
};
use crate::security::SecurityConfig;

pub enum MiddlewareInst {
	L4Peek(Arc<dyn L4PeekMiddleware>),
	L4Bytes(Arc<dyn L4BytesMiddleware>),
	L7Request(Arc<dyn L7RequestMiddleware>),
	L7Response(Arc<dyn L7ResponseMiddleware>),
	// TODO: S3 adds `Wasm(WasmMiddleware)` once the WASM host lands (see
	// 04-middleware.md § _`WasmMiddleware` shape_).
}

impl MiddlewareInst {
	#[must_use]
	pub const fn kind(&self) -> MiddlewareKind {
		match self {
			Self::L4Peek(_) => MiddlewareKind::L4Peek,
			Self::L4Bytes(_) => MiddlewareKind::L4Bytes,
			Self::L7Request(_) => MiddlewareKind::L7Request,
			Self::L7Response(_) => MiddlewareKind::L7Response,
		}
	}
}

pub enum FetchInst {
	L4(Arc<dyn L4Fetch>),
	L7(Arc<dyn L7Fetch>),
}

pub struct FlowGraph {
	symbolic: Arc<SymbolicFlowGraph>,
	middlewares: Vec<MiddlewareInst>,
	fetches: Vec<FetchInst>,
	meta: FlowGraphMeta,
	/// Per-listener parsed TLS server config. Populated from
	/// `sym.meta.listener_tls`'s symbolic PEM paths during [`Self::link`];
	/// the listener accept loop looks up the bind address here on each
	/// accepted connection and, if `Some`, runs a server-side handshake
	/// before passing the wrapped stream to the executor as
	/// [`vane_core::L4Conn::Tls`]. See `spec/architecture/08-tls.md`
	/// § _TLS termination (L4 → L7 upgrade)_.
	listener_tls: BTreeMap<SocketAddr, Arc<rustls::ServerConfig>>,
	/// L1 security config available to the executor (H1/H2 builder
	/// configuration, header size/count limits). Derived at link time
	/// from the daemon's env; default values used for test graphs that
	/// don't need floor-enforcement tuning.
	security_cfg: Arc<SecurityConfig>,
}

impl FlowGraph {
	#[must_use]
	pub fn symbolic(&self) -> &Arc<SymbolicFlowGraph> {
		&self.symbolic
	}

	#[must_use]
	pub fn meta(&self) -> &FlowGraphMeta {
		&self.meta
	}

	/// L1 security configuration for this graph. Used by the H1/H2
	/// server builders in `upgrade.rs` to configure header limits and
	/// timeout. Tests that call `FlowGraph::link` get `SecurityConfig::default()`.
	#[must_use]
	pub fn security_cfg(&self) -> &Arc<SecurityConfig> {
		&self.security_cfg
	}

	/// Per-listener parsed TLS server config. `None` for cleartext
	/// listeners. Looked up by bind address in the accept loop.
	#[must_use]
	pub fn listener_tls(&self, addr: &SocketAddr) -> Option<&Arc<rustls::ServerConfig>> {
		self.listener_tls.get(addr)
	}

	/// Reachability check: does any node walked from `entry` reference an
	/// `L4Peek` middleware? The listener uses this at start-up to decide
	/// whether to enable the peek prelude (the slow path that reads up
	/// to 8 KiB into a buffer before dispatching). When the answer is
	/// `false`, accept stays on the zero-copy fast path.
	///
	/// BFS with a `visited` set so future graph shapes that introduce
	/// cycles (none today — `link` rejects them) cannot trip an infinite
	/// loop here.
	#[must_use]
	pub fn needs_peek(&self, entry: NodeId) -> bool {
		let sym = self.symbolic.as_ref();
		let mut visited: HashSet<NodeId> = HashSet::new();
		let mut queue: VecDeque<NodeId> = VecDeque::new();
		queue.push_back(entry);
		while let Some(node_id) = queue.pop_front() {
			if !visited.insert(node_id) {
				continue;
			}
			let Some(node) = sym.nodes.get(node_id.get() as usize) else {
				continue;
			};
			match node {
				Node::Check { on_match, on_miss, .. } => {
					queue.push_back(*on_match);
					queue.push_back(*on_miss);
				}
				Node::Middleware { id, next, on_error, .. } => {
					let kind = sym.middlewares[id.get() as usize].kind;
					if kind == MiddlewareKind::L4Peek {
						return true;
					}
					queue.push_back(*next);
					if let Some(e) = on_error {
						queue.push_back(*e);
					}
				}
				Node::Fetch { next_response, next_tunnel, .. } => {
					if let Some(n) = next_response {
						queue.push_back(*n);
					}
					if let Some(n) = next_tunnel {
						queue.push_back(*n);
					}
				}
				Node::Upgrade { next } => queue.push_back(*next),
				Node::Terminate(_) => {}
			}
		}
		false
	}

	/// Resolve every `SymbolicMiddlewareRef` / `SymbolicFetchRef` against
	/// the factory registries, construct `Arc<dyn Trait>` values, and emit
	/// the runtime `FlowGraph`. See 02-flow.md § _link_.
	///
	/// Uses `SecurityConfig::default()` for the L1 floor config.
	/// Production callers that have validated env-var values use
	/// [`Self::link_with_security`] instead.
	///
	/// # Errors
	/// Returns [`LinkError`] on any of: unknown middleware name, unknown
	/// fetch kind, factory-rejected args, middleware kind mismatch
	/// (declared vs produced), or feature-gated factory reached by a
	/// rule referencing a capability the binary was not built with.
	pub fn link(
		sym: Arc<SymbolicFlowGraph>,
		mw_factories: &MiddlewareFactories,
		fetch_factories: &FetchFactories,
	) -> Result<Arc<Self>, LinkError> {
		Self::link_inner(sym, mw_factories, fetch_factories, Arc::new(SecurityConfig::default()))
	}

	/// Like [`Self::link`] but with an explicit [`SecurityConfig`]
	/// (floor-validated by the caller via [`SecurityConfig::new`]).
	/// Production daemon code uses this path; tests use [`Self::link`]
	/// and get `SecurityConfig::default()`.
	///
	/// # Errors
	/// Same as [`Self::link`].
	pub fn link_with_security(
		sym: Arc<SymbolicFlowGraph>,
		mw_factories: &MiddlewareFactories,
		fetch_factories: &FetchFactories,
		security_cfg: Arc<SecurityConfig>,
	) -> Result<Arc<Self>, LinkError> {
		Self::link_inner(sym, mw_factories, fetch_factories, security_cfg)
	}

	fn link_inner(
		sym: Arc<SymbolicFlowGraph>,
		mw_factories: &MiddlewareFactories,
		fetch_factories: &FetchFactories,
		security_cfg: Arc<SecurityConfig>,
	) -> Result<Arc<Self>, LinkError> {
		let mut middlewares = Vec::with_capacity(sym.middlewares.len());
		for symref in &sym.middlewares {
			let entry = mw_factories
				.get(symref.name.as_ref())
				.ok_or_else(|| LinkError::UnknownMiddleware(Arc::clone(&symref.name)))?;
			let inst = match entry {
				MiddlewareFactoryEntry::FeatureGated(feature) => {
					return Err(LinkError::FeatureDisabled { feature });
				}
				MiddlewareFactoryEntry::Available { kind, construct } => {
					let built = construct(&symref.args).map_err(|e: FactoryError| {
						LinkError::MiddlewareFactoryRejected { name: Arc::clone(&symref.name), cause: e.0 }
					})?;
					let produced = built.kind();
					if symref.kind != *kind || symref.kind != produced {
						return Err(LinkError::MiddlewareKindMismatch {
							name: Arc::clone(&symref.name),
							declared: symref.kind,
							produced,
						});
					}
					built
				}
			};
			middlewares.push(inst);
		}

		let mut fetches = Vec::with_capacity(sym.fetches.len());
		for symref in &sym.fetches {
			let entry = fetch_factories.get(symref.kind).ok_or(LinkError::UnknownFetch(symref.kind))?;
			let inst = match entry {
				FetchFactoryEntry::FeatureGated(feature) => {
					return Err(LinkError::FeatureDisabled { feature });
				}
				FetchFactoryEntry::Available(construct) => {
					construct(&symref.args).map_err(|e: FactoryError| LinkError::FetchFactoryRejected {
						kind: symref.kind,
						cause: e.0,
					})?
				}
			};
			fetches.push(inst);
		}

		// Parse every symbolic-meta `listener_tls` entry into a
		// `rustls::ServerConfig`. PEM I/O happens here (link stage) so a
		// missing or malformed cert/key is caught at config-load time
		// rather than per-accept. See 08-tls.md § _TLS termination
		// (L4 → L7 upgrade)_ and § _Certificate resolver_.
		let mut listener_tls: BTreeMap<SocketAddr, Arc<rustls::ServerConfig>> = BTreeMap::new();
		for (addr, spec) in &sym.meta.listener_tls {
			let server_config = build_listener_server_config(spec)
				.map_err(|cause| LinkError::TlsConfig { addr: *addr, cause })?;
			listener_tls.insert(*addr, Arc::new(server_config));
		}

		// Inherit version_hash / compiled_at / source_files from the symbolic
		// meta; overwrite feature_set with this binary's snapshot per 02-flow.md
		// § _FlowGraph metadata_ — `feature_set` is "what the daemon linked",
		// not "what the rule-set intended".
		let meta = FlowGraphMeta {
			version_hash: sym.meta.version_hash,
			compiled_at: sym.meta.compiled_at,
			source_files: sym.meta.source_files.clone(),
			feature_set: crate::ENGINE_FEATURE_SET,
			short_circuit_response_entry: sym.meta.short_circuit_response_entry.clone(),
			listener_tls: sym.meta.listener_tls.clone(),
		};

		Ok(Arc::new(Self { symbolic: sym, middlewares, fetches, meta, listener_tls, security_cfg }))
	}
}

/// Build a `rustls::ServerConfig` for a listener's cert pool. ALPN
/// advertises `["h2", "http/1.1"]`; the executor's `Node::Upgrade` arm
/// dispatches to `drive_h2_server` or `drive_h1_server` based on the
/// negotiated ALPN. SNI dispatch lives in [`SniResolver::resolve`].
fn build_listener_server_config(spec: &ListenerTlsSpec) -> Result<rustls::ServerConfig, String> {
	let default = spec.default.as_ref().map(load_certified_key).transpose()?.map(Arc::new);

	let mut by_sni: HashMap<String, Arc<rustls::sign::CertifiedKey>> = HashMap::new();
	for (sni, tls) in &spec.sni_certs {
		let ck = load_certified_key(tls)?;
		// Lower has already lowercased the key — assert it as a
		// belt-and-suspenders for any post-lower meta tampering.
		debug_assert_eq!(sni, &sni.to_ascii_lowercase());
		by_sni.insert(sni.clone(), Arc::new(ck));
	}

	if default.is_none() && by_sni.is_empty() {
		return Err("listener TLS spec is empty (no default + no sni certs)".to_owned());
	}

	let resolver = Arc::new(SniResolver { by_sni, default });

	let mut server_config =
		rustls::ServerConfig::builder().with_no_client_auth().with_cert_resolver(resolver);
	// Two-protocol ALPN — h2 preferred, http/1.1 fallback. The executor's
	// Upgrade arm reads the negotiated protocol off `ConnContext.tls.alpn`
	// and routes to the matching driver.
	server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
	Ok(server_config)
}

/// Custom rustls cert resolver: SNI lookup with default-cert fallback.
///
/// rustls 0.23's built-in `ResolvesServerCertUsingSni` errors on
/// unmatched SNI rather than falling back; spec 08-tls.md
/// § _Certificate resolver_ requires a default-cert fallback for
/// `ClientHello`s with missing or unknown SNI, so we wrap the lookup
/// with our own resolver.
#[derive(Debug)]
struct SniResolver {
	by_sni: HashMap<String, Arc<rustls::sign::CertifiedKey>>,
	default: Option<Arc<rustls::sign::CertifiedKey>>,
}

impl rustls::server::ResolvesServerCert for SniResolver {
	fn resolve(
		&self,
		hello: rustls::server::ClientHello<'_>,
	) -> Option<Arc<rustls::sign::CertifiedKey>> {
		// `server_name()` is already ASCII-lowercased by rustls per
		// RFC 6066 § 3, so a direct map lookup suffices.
		if let Some(sni) = hello.server_name()
			&& let Some(ck) = self.by_sni.get(sni)
		{
			return Some(Arc::clone(ck));
		}
		self.default.clone()
	}
}

/// Read PEM, parse cert chain + private key, sign with the installed
/// crypto provider's key loader. Errors are stringly-typed so the
/// caller can wrap with `LinkError::TlsConfig { addr, cause }`.
fn load_certified_key(tls: &TlsConfig) -> Result<rustls::sign::CertifiedKey, String> {
	let cert_bytes = fs::read(&tls.cert_file)
		.map_err(|e| format!("read cert_file {}: {e}", tls.cert_file.display()))?;
	let key_bytes = fs::read(&tls.key_file)
		.map_err(|e| format!("read key_file {}: {e}", tls.key_file.display()))?;

	let cert_chain: Vec<rustls::pki_types::CertificateDer<'static>> =
		rustls_pemfile::certs(&mut cert_bytes.as_slice())
			.collect::<Result<_, _>>()
			.map_err(|e| format!("parse cert_file {}: {e}", tls.cert_file.display()))?;
	if cert_chain.is_empty() {
		return Err(format!("cert_file {} contained no certificates", tls.cert_file.display()));
	}

	let private_key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
		.map_err(|e| format!("parse key_file {}: {e}", tls.key_file.display()))?
		.ok_or_else(|| format!("key_file {} contained no private key", tls.key_file.display()))?;

	let provider = rustls::crypto::CryptoProvider::get_default()
		.ok_or_else(|| "rustls crypto provider not installed".to_owned())?;
	let signing_key = provider
		.key_provider
		.load_private_key(private_key)
		.map_err(|e| format!("load_private_key {}: {e}", tls.key_file.display()))?;

	Ok(rustls::sign::CertifiedKey::new(cert_chain, signing_key))
}

impl Index<MiddlewareId> for FlowGraph {
	type Output = MiddlewareInst;
	fn index(&self, id: MiddlewareId) -> &MiddlewareInst {
		&self.middlewares[id.get() as usize]
	}
}

impl Index<FetchId> for FlowGraph {
	type Output = FetchInst;
	fn index(&self, id: FetchId) -> &FetchInst {
		&self.fetches[id.get() as usize]
	}
}

#[derive(thiserror::Error, Debug)]
pub enum LinkError {
	#[error("unknown middleware name {0:?} — no factory registered in this binary")]
	UnknownMiddleware(Arc<str>),

	#[error("unknown fetch kind {0:?} — no factory registered in this binary")]
	UnknownFetch(FetchKind),

	#[error("middleware {name:?} factory produced kind {produced:?}, declared kind {declared:?}")]
	MiddlewareKindMismatch { name: Arc<str>, declared: MiddlewareKind, produced: MiddlewareKind },

	#[error("middleware {name:?}: {cause}")]
	MiddlewareFactoryRejected { name: Arc<str>, cause: String },

	#[error("fetch {kind:?}: {cause}")]
	FetchFactoryRejected { kind: FetchKind, cause: String },

	// Spec 02-flow.md § _link_ (line 111) pins the wording:
	//   "this binary was built without the 'h3' feature — rebuild with
	//    --features h3 or remove the rule"
	// single quotes around the feature name. (The C6 task prompt used
	// double quotes in its example; flagged as SPEC DEVIATION in the
	// chunk report. Spec wins.)
	#[error(
		"this binary was built without the '{feature}' feature — rebuild with --features {feature} or remove the rule"
	)]
	FeatureDisabled { feature: &'static str },

	#[error("listener {addr} TLS config: {cause}")]
	TlsConfig { addr: SocketAddr, cause: String },
}

#[cfg(test)]
mod tests {
	use std::io::Write as _;

	use tempfile::NamedTempFile;

	use super::*;

	fn install_crypto_for_test() {
		crate::crypto::install_default_provider();
	}

	fn write_pem(contents: &str) -> NamedTempFile {
		let mut f = NamedTempFile::new().expect("tmpfile");
		f.write_all(contents.as_bytes()).expect("write pem");
		f
	}

	fn rcgen_self_signed() -> (String, String) {
		let issued =
			rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("self-signed cert");
		(issued.cert.pem(), issued.signing_key.serialize_pem())
	}

	fn default_only_spec(
		cert: NamedTempFile,
		key: NamedTempFile,
	) -> (ListenerTlsSpec, NamedTempFile, NamedTempFile) {
		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: cert.path().to_path_buf(),
				key_file: key.path().to_path_buf(),
			}),
			sni_certs: BTreeMap::new(),
		};
		(spec, cert, key)
	}

	#[test]
	fn build_listener_server_config_advertises_h2_and_h1_alpn() {
		install_crypto_for_test();
		let (cert_pem, key_pem) = rcgen_self_signed();
		let cert_file = write_pem(&cert_pem);
		let key_file = write_pem(&key_pem);
		let (spec, _cert, _key) = default_only_spec(cert_file, key_file);
		let server = build_listener_server_config(&spec).expect("build_listener_server_config");
		assert_eq!(server.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
	}

	#[test]
	fn build_listener_server_config_errors_when_cert_file_missing() {
		install_crypto_for_test();
		let (_, key_pem) = rcgen_self_signed();
		let key_file = write_pem(&key_pem);
		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: "/nonexistent/path/to/cert.pem".into(),
				key_file: key_file.path().to_path_buf(),
			}),
			sni_certs: BTreeMap::new(),
		};
		let err = build_listener_server_config(&spec).expect_err("missing cert must error");
		assert!(err.contains("read cert_file"), "error mentions cert_file read failure: {err}");
	}

	#[test]
	fn build_listener_server_config_errors_on_garbage_cert_pem() {
		install_crypto_for_test();
		let (_, key_pem) = rcgen_self_signed();
		let cert_file = write_pem("this is not a PEM cert\n");
		let key_file = write_pem(&key_pem);
		let spec = ListenerTlsSpec {
			default: Some(TlsConfig {
				sni: None,
				cert_file: cert_file.path().to_path_buf(),
				key_file: key_file.path().to_path_buf(),
			}),
			sni_certs: BTreeMap::new(),
		};
		let err = build_listener_server_config(&spec).expect_err("garbage cert must error");
		assert!(
			err.contains("contained no certificates") || err.contains("parse cert_file"),
			"error explains the cert parse failure: {err}",
		);
	}

	mod needs_peek_tests {
		use std::time::SystemTime;

		use serde_json::json;
		use vane_core::{
			PredicateId, PredicateInst, SymbolicMiddlewareRef, Terminator, TerminatorId,
			predicate::{CompiledOperator, CompiledValue, FieldPath},
		};

		use super::*;

		fn dummy_meta() -> FlowGraphMeta {
			FlowGraphMeta {
				version_hash: [0u8; 32],
				compiled_at: SystemTime::UNIX_EPOCH,
				source_files: Vec::new(),
				feature_set: &[],
				short_circuit_response_entry: BTreeMap::new(),
				listener_tls: BTreeMap::new(),
			}
		}

		fn mw_ref(name: &str, kind: MiddlewareKind) -> SymbolicMiddlewareRef {
			SymbolicMiddlewareRef {
				name: Arc::from(name),
				args: json!({}),
				kind,
				stateless: true,
				needs_body: false,
				on_error: None,
			}
		}

		/// Build a [`FlowGraph`] from a hand-rolled symbolic graph,
		/// skipping the factory-driven `link` path. Tests for the
		/// reachability walker only need accurate node and middleware
		/// metadata; the actual `MiddlewareInst` / `FetchInst` slots stay
		/// empty because `needs_peek` reads kinds from the symbolic refs.
		fn flow_from_with(
			nodes: Vec<Node>,
			middlewares: Vec<SymbolicMiddlewareRef>,
			predicates: Vec<PredicateInst>,
		) -> FlowGraph {
			let sym = SymbolicFlowGraph {
				nodes,
				predicates,
				middlewares,
				fetches: Vec::new(),
				terminators: vec![Terminator::Close],
				entries: HashMap::new(),
				meta: dummy_meta(),
			};
			FlowGraph {
				symbolic: Arc::new(sym),
				middlewares: Vec::new(),
				fetches: Vec::new(),
				meta: dummy_meta(),
				listener_tls: BTreeMap::new(),
				security_cfg: Arc::new(SecurityConfig::default()),
			}
		}

		fn flow_from(nodes: Vec<Node>, middlewares: Vec<SymbolicMiddlewareRef>) -> FlowGraph {
			flow_from_with(nodes, middlewares, Vec::new())
		}

		#[test]
		fn needs_peek_false_when_entry_runs_through_upgrade_to_terminator() {
			// entry → Upgrade → Terminate. No L4Peek anywhere.
			let nodes =
				vec![Node::Upgrade { next: NodeId::new(1) }, Node::Terminate(TerminatorId::new(0))];
			let g = flow_from(nodes, Vec::new());
			assert!(!g.needs_peek(NodeId::new(0)));
		}

		#[test]
		fn needs_peek_true_when_l4peek_middleware_is_directly_reachable() {
			// entry → L4Peek middleware → Terminate.
			let nodes = vec![
				Node::Middleware {
					id: MiddlewareId::new(0),
					next: NodeId::new(1),
					on_error: None,
					collect_body_before: None,
					body_limit: 0,
				},
				Node::Terminate(TerminatorId::new(0)),
			];
			let mws = vec![mw_ref("sni_peek", MiddlewareKind::L4Peek)];
			let g = flow_from(nodes, mws);
			assert!(g.needs_peek(NodeId::new(0)));
		}

		#[test]
		fn needs_peek_false_when_only_non_peek_middleware_is_reachable() {
			let nodes = vec![
				Node::Middleware {
					id: MiddlewareId::new(0),
					next: NodeId::new(1),
					on_error: None,
					collect_body_before: None,
					body_limit: 0,
				},
				Node::Terminate(TerminatorId::new(0)),
			];
			let mws = vec![mw_ref("rate_limit", MiddlewareKind::L7Request)];
			let g = flow_from(nodes, mws);
			assert!(!g.needs_peek(NodeId::new(0)));
		}

		#[test]
		fn needs_peek_true_when_check_branch_contains_l4peek() {
			// entry (Check) → on_match: Terminate, on_miss: L4Peek → Terminate.
			let nodes = vec![
				Node::Check {
					predicate: PredicateId::new(0),
					on_match: NodeId::new(1),
					on_miss: NodeId::new(2),
					collect_body_before: None,
					body_limit: 0,
				},
				Node::Terminate(TerminatorId::new(0)),
				Node::Middleware {
					id: MiddlewareId::new(0),
					next: NodeId::new(1),
					on_error: None,
					collect_body_before: None,
					body_limit: 0,
				},
			];
			let mws = vec![mw_ref("sni_peek", MiddlewareKind::L4Peek)];
			let predicates = vec![PredicateInst {
				path: FieldPath::Transport,
				op: CompiledOperator::Equals(CompiledValue::Str(Arc::from("tcp"))),
			}];
			let g = flow_from_with(nodes, mws, predicates);
			assert!(g.needs_peek(NodeId::new(0)));
		}

		#[test]
		fn needs_peek_visited_set_handles_self_loop_without_diverging() {
			// Pathological: a Middleware whose `next` points back to itself.
			// `link` rejects this in production, but the walker must not
			// hang if a future graph mutation slips one through.
			let nodes = vec![Node::Middleware {
				id: MiddlewareId::new(0),
				next: NodeId::new(0),
				on_error: None,
				collect_body_before: None,
				body_limit: 0,
			}];
			let mws = vec![mw_ref("rate_limit", MiddlewareKind::L7Request)];
			let g = flow_from(nodes, mws);
			assert!(!g.needs_peek(NodeId::new(0)));
		}
	}
}
