use std::ops::Index;
use std::sync::Arc;

use vane_core::{
	FetchId, FetchKind, FlowGraphMeta, L4BytesMiddleware, L4Fetch, L4PeekMiddleware, L7Fetch,
	L7RequestMiddleware, L7ResponseMiddleware, MiddlewareId, MiddlewareKind, SymbolicFlowGraph,
};

use crate::factories::{
	FactoryError, FetchFactories, FetchFactoryEntry, MiddlewareFactories, MiddlewareFactoryEntry,
};

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

	/// Resolve every `SymbolicMiddlewareRef` / `SymbolicFetchRef` against
	/// the factory registries, construct `Arc<dyn Trait>` values, and emit
	/// the runtime `FlowGraph`. See 02-flow.md § _link_.
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

		// Inherit version_hash / compiled_at / source_files from the symbolic
		// meta; overwrite feature_set with this binary's snapshot per 02-flow.md
		// § _FlowGraph metadata_ — `feature_set` is "what the daemon linked",
		// not "what the rule-set intended".
		let meta = FlowGraphMeta {
			version_hash: sym.meta.version_hash,
			compiled_at: sym.meta.compiled_at,
			source_files: sym.meta.source_files.clone(),
			feature_set: crate::ENGINE_FEATURE_SET,
		};

		Ok(Arc::new(Self { symbolic: sym, middlewares, fetches, meta }))
	}
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
}
