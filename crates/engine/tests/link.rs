//! Integration tests for `vane_engine::flow_graph::FlowGraph::link`.
//!
//! Covers the link-pass contract described in `spec/flow-model.md`
//! § _Compile and link — two stages, two crates_ / _The compiled form_ and
//! `spec/crates/core.md` § _Listener kind derivation_. Each test hand-builds a minimal `SymbolicFlowGraph` rather
//! than routing through `vane_core::compile`, so that the link pass is
//! exercised in isolation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use serde_json::Value;
use vane_core::{
	ConnContext, Decision, Error, FetchId, FetchKind, FlowCtx, FlowGraphMeta, L4BytesMiddleware,
	L4Conn, L4PeekMiddleware, L7RequestMiddleware, L7ResponseMiddleware, MiddlewareId,
	MiddlewareKind, Request, Response, SymbolicFetchRef, SymbolicFlowGraph, SymbolicMiddlewareRef,
};
use vane_engine::factories::{FactoryError, FetchFactories, MiddlewareFactories};
use vane_engine::flow_graph::{FetchInst, FlowGraph, LinkError, MiddlewareInst};

// Dummy middleware impls. Link tests never drive these; `run` is unreachable.

struct NoopL4Peek;
#[async_trait]
impl L4PeekMiddleware for NoopL4Peek {
	async fn run(
		&self,
		_peek: &[u8],
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		unreachable!("link tests never drive middleware")
	}
}

struct NoopL4Bytes;
#[async_trait]
impl L4BytesMiddleware for NoopL4Bytes {
	async fn run(
		&self,
		_l4: &mut L4Conn,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		unreachable!("link tests never drive middleware")
	}
}

struct NoopL7Req;
#[async_trait]
impl L7RequestMiddleware for NoopL7Req {
	async fn run(
		&self,
		_req: &mut Request,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		unreachable!("link tests never drive middleware")
	}
}

struct NoopL7Resp;
#[async_trait]
impl L7ResponseMiddleware for NoopL7Resp {
	async fn run(
		&self,
		_resp: &mut Response,
		_conn: &Arc<ConnContext>,
		_ctx: &mut FlowCtx,
	) -> Result<Decision, Error> {
		unreachable!("link tests never drive middleware")
	}
}

// Graph construction helpers — zero-node graphs with exactly the slabs each
// test needs. `feature_set: &[]` mimics core's pre-link state; link must
// overwrite it with the engine's `ENGINE_FEATURE_SET`.

fn sample_meta() -> FlowGraphMeta {
	FlowGraphMeta {
		version_hash: [0; 32],
		compiled_at: SystemTime::UNIX_EPOCH,
		source_files: vec![],
		feature_set: &[],
		short_circuit_response_entry: std::collections::BTreeMap::new(),
		listener_tls: std::collections::BTreeMap::new(),
		listener_kinds: std::collections::BTreeMap::new(),

		listener_transports: std::collections::BTreeMap::new(),
		annotations: Vec::new(),
	}
}

fn empty_graph() -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes: vec![],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![],
		terminators: vec![],
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn graph_with_middleware(mref: SymbolicMiddlewareRef) -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes: vec![],
		predicates: vec![],
		middlewares: vec![mref],
		fetches: vec![],
		terminators: vec![],
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn graph_with_fetch(fref: SymbolicFetchRef) -> Arc<SymbolicFlowGraph> {
	Arc::new(SymbolicFlowGraph {
		nodes: vec![],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![fref],
		terminators: vec![],
		entries: HashMap::new(),
		meta: sample_meta(),
	})
}

fn l7_req_ref(name: &str) -> SymbolicMiddlewareRef {
	SymbolicMiddlewareRef {
		name: Arc::from(name),
		args: Value::Null,
		kind: MiddlewareKind::L7Request,
		stateless: true,
		needs_body: false,
		on_error: None,
	}
}

// 1. link_empty_graph_succeeds

#[test]
fn link_empty_graph_succeeds() {
	let sym = empty_graph();
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let linked = FlowGraph::link(sym, &mw, &fetch).expect("link empty graph");
	// Spec spec/flow-model.md § _The compiled form_: link installs the engine's
	// feature-set snapshot. Core's crypto backend leads per lib.rs:43.
	assert!(
		linked.meta().feature_set.contains(&vane_engine::crypto::BACKEND_NAME),
		"expected feature_set to contain crypto backend {:?}, got {:?}",
		vane_engine::crypto::BACKEND_NAME,
		linked.meta().feature_set,
	);
}

// 2. link_fails_on_unknown_middleware_name

#[test]
fn link_fails_on_unknown_middleware_name() {
	let sym = graph_with_middleware(l7_req_ref("does_not_exist"));
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let Err(err) = FlowGraph::link(sym, &mw, &fetch) else {
		panic!("unknown middleware must fail link")
	};
	assert!(
		matches!(err, LinkError::UnknownMiddleware(_)),
		"expected UnknownMiddleware, got {err:?}",
	);
	let rendered = err.to_string();
	assert!(
		rendered.contains("does_not_exist"),
		"expected error display to mention the missing name, got {rendered:?}",
	);
}

// 3. link_fails_on_unknown_fetch_kind

#[test]
fn link_fails_on_unknown_fetch_kind() {
	let sym = graph_with_fetch(SymbolicFetchRef {
		kind: FetchKind::HttpProxy,
		args: Value::Null,
		retry_buffer_required: false,
		allow_zero_rtt: None,
	});
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let Err(err) = FlowGraph::link(sym, &mw, &fetch) else {
		panic!("unknown fetch kind must fail link")
	};
	match err {
		LinkError::UnknownFetch(kind) => assert_eq!(kind, FetchKind::HttpProxy),
		other => panic!("expected UnknownFetch(HttpProxy), got {other:?}"),
	}
}

// 4. link_fails_on_factory_args_rejection

#[test]
fn link_fails_on_factory_args_rejection() {
	let sym = graph_with_middleware(l7_req_ref("m"));
	let mut mw = MiddlewareFactories::new();
	mw.register("m", MiddlewareKind::L7Request, |_args| {
		Err(FactoryError::Invalid("bad args".into()))
	});
	let fetch = FetchFactories::new();
	let Err(err) = FlowGraph::link(sym, &mw, &fetch) else {
		panic!("factory rejection must fail link")
	};
	match err {
		LinkError::MiddlewareFactoryRejected { name, cause } => {
			assert_eq!(name.as_ref(), "m");
			assert_eq!(cause, "bad args");
		}
		other => panic!("expected MiddlewareFactoryRejected, got {other:?}"),
	}
}

// 5. link_fails_on_kind_mismatch

#[test]
fn link_fails_on_kind_mismatch() {
	// Symbolic ref declares L7Request; registry declares L7Request too
	// (so the registry-vs-declared check passes); but the closure produces
	// an L4Peek variant. The link pass must catch the variant-vs-declared
	// mismatch per spec/crates/engine.md § _Middleware_ (the variant *is*
	// the kind).
	let sym = graph_with_middleware(l7_req_ref("m"));
	let mut mw = MiddlewareFactories::new();
	mw.register("m", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L4Peek(Arc::new(NoopL4Peek)))
	});
	let fetch = FetchFactories::new();
	let Err(err) = FlowGraph::link(sym, &mw, &fetch) else { panic!("kind mismatch must fail link") };
	match err {
		LinkError::MiddlewareKindMismatch { name, declared, produced } => {
			assert_eq!(name.as_ref(), "m");
			assert_eq!(declared, MiddlewareKind::L7Request);
			assert_eq!(produced, MiddlewareKind::L4Peek);
		}
		other => panic!("expected MiddlewareKindMismatch, got {other:?}"),
	}
}

// 6. link_fails_on_feature_disabled_with_spec_message

#[test]
fn link_fails_on_feature_disabled_with_spec_message() {
	// spec/flow-model.md § _Compile and link — two stages, two crates_ pins the phrasing exactly,
	// with *single quotes* around the feature name.
	let sym = graph_with_middleware(l7_req_ref("http_upstream_cgi"));
	let mut mw = MiddlewareFactories::new();
	mw.register_feature_gated("http_upstream_cgi", "cgi");
	let fetch = FetchFactories::new();
	let Err(err) = FlowGraph::link(sym, &mw, &fetch) else {
		panic!("feature-gated middleware must fail link")
	};
	match &err {
		LinkError::FeatureDisabled { feature } => assert_eq!(*feature, "cgi"),
		other => panic!("expected FeatureDisabled, got {other:?}"),
	}
	assert_eq!(
		err.to_string(),
		"this binary was built without the 'cgi' feature — rebuild with --features cgi or remove the rule",
	);
}

// 7. link_preserves_version_hash_but_overrides_feature_set

#[test]
fn link_preserves_version_hash_but_overrides_feature_set() {
	let stale: &'static [&'static str] = &["stale"];
	let sym = Arc::new(SymbolicFlowGraph {
		nodes: vec![],
		predicates: vec![],
		middlewares: vec![],
		fetches: vec![],
		terminators: vec![],
		entries: HashMap::new(),
		meta: FlowGraphMeta {
			version_hash: [7; 32],
			compiled_at: SystemTime::UNIX_EPOCH,
			source_files: vec![],
			feature_set: stale,
			short_circuit_response_entry: std::collections::BTreeMap::new(),
			listener_tls: std::collections::BTreeMap::new(),
			listener_kinds: std::collections::BTreeMap::new(),

			listener_transports: std::collections::BTreeMap::new(),
			annotations: Vec::new(),
		},
	});
	let mw = MiddlewareFactories::new();
	let fetch = FetchFactories::new();
	let linked = FlowGraph::link(sym, &mw, &fetch).expect("link must succeed");
	assert_eq!(linked.meta().version_hash, [7; 32]);
	assert_ne!(
		linked.meta().feature_set,
		stale,
		"link must overwrite feature_set with ENGINE_FEATURE_SET",
	);
	assert!(
		linked.meta().feature_set.contains(&vane_engine::crypto::BACKEND_NAME),
		"expected engine's ENGINE_FEATURE_SET to contain the crypto backend, got {:?}",
		linked.meta().feature_set,
	);
}

// 8a. index_middleware_id_returns_inst

#[test]
fn index_middleware_id_returns_inst() {
	let sym = graph_with_middleware(l7_req_ref("m"));
	let mut mw = MiddlewareFactories::new();
	mw.register("m", MiddlewareKind::L7Request, |_args| {
		Ok(MiddlewareInst::L7Request(Arc::new(NoopL7Req)))
	});
	let fetch = FetchFactories::new();
	let linked = FlowGraph::link(sym, &mw, &fetch).expect("link must succeed");
	assert!(
		matches!(&linked[MiddlewareId::for_testing(0)], MiddlewareInst::L7Request(_)),
		"expected index to return the L7Request variant produced by the factory",
	);
}

// 8b. index_fetch_id_returns_inst

#[test]
fn index_fetch_id_returns_inst() {
	// Minimal L7Fetch impl — unreachable because link tests do not drive fetches.
	struct NoopL7Fetch;
	#[async_trait]
	impl vane_core::L7Fetch for NoopL7Fetch {
		async fn fetch(
			&self,
			_req: Request,
			_conn: &Arc<ConnContext>,
			_ctx: &mut FlowCtx,
		) -> Result<vane_core::L7FetchOutput, Error> {
			unreachable!("link tests never drive fetches")
		}
	}
	let sym = graph_with_fetch(SymbolicFetchRef {
		kind: FetchKind::HttpProxy,
		args: Value::Null,
		retry_buffer_required: false,
		allow_zero_rtt: None,
	});
	let mw = MiddlewareFactories::new();
	let mut fetch = FetchFactories::new();
	fetch.register(FetchKind::HttpProxy, |_args| Ok(FetchInst::L7(Arc::new(NoopL7Fetch))));
	let linked = FlowGraph::link(sym, &mw, &fetch).expect("link must succeed");
	assert!(
		matches!(&linked[FetchId::for_testing(0)], FetchInst::L7(_)),
		"expected index to return the L7 fetch variant produced by the factory",
	);
}

// Keep the dummy L4 bytes middleware referenced so the file compiles without
// warnings even while its variant is only used by coverage proxies.
#[allow(dead_code)]
fn _ensure_l4_bytes_constructs() -> MiddlewareInst {
	MiddlewareInst::L4Bytes(Arc::new(NoopL4Bytes))
}

#[allow(dead_code)]
fn _ensure_l7_resp_constructs() -> MiddlewareInst {
	MiddlewareInst::L7Response(Arc::new(NoopL7Resp))
}
