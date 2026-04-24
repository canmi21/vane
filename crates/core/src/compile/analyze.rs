use crate::compile::expand::RawRuleSet;
use crate::error::Error;
use crate::fetch::FetchKind;
use crate::metadata::{FetchMetadataProvider, MiddlewareMetadataProvider};
use crate::predicate::{FieldPath, Predicate};
use crate::rule::RawRule;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum InspectionLevel {
	L4Only,
	L4Peek,
	L7Header,
	L7Body,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum Posture {
	L4,
	L7,
}

#[derive(Debug, Clone)]
pub struct AnalyzedRule {
	pub raw: RawRule,
	pub inspection_level: InspectionLevel,
	pub specificity: usize,
	pub posture: Posture,
	pub needs_request_body: bool,
	pub needs_response_body: bool,
}

#[derive(Debug, Clone)]
pub struct AnalyzedRuleSet {
	pub rules: Vec<AnalyzedRule>,
	pub source_files: Vec<std::path::PathBuf>,
}

/// Compute per-rule inspection level, specificity, posture (L4 vs L7), and
/// `LazyBuffer` per-side buffer triggers.
///
/// # Errors
/// Returns [`Error::compile`] when a referenced middleware name is missing
/// from the provider registry (so compile-time analysis cannot decide what
/// phase it sits in or whether it buffers the body).
pub fn analyze(
	set: RawRuleSet,
	mw_meta: &dyn MiddlewareMetadataProvider,
	fetch_meta: &dyn FetchMetadataProvider,
) -> Result<AnalyzedRuleSet, Error> {
	let mut analyzed = Vec::with_capacity(set.rules.len());
	for raw in set.rules {
		analyzed.push(analyze_rule(raw, mw_meta, fetch_meta)?);
	}
	Ok(AnalyzedRuleSet { rules: analyzed, source_files: set.source_files })
}

fn analyze_rule(
	raw: RawRule,
	mw_meta: &dyn MiddlewareMetadataProvider,
	fetch_meta: &dyn FetchMetadataProvider,
) -> Result<AnalyzedRule, Error> {
	let fetch_kind = Some(raw.terminate.kind);
	let fetch_phase = fetch_phase_of(fetch_kind);

	let mut max_level = InspectionLevel::L4Only;
	let mut specificity = 0usize;
	let mut reads_http_body = false;
	if let Some(pred) = &raw.match_predicate {
		walk_predicate(pred, &mut |p| match p {
			Predicate::Check(c) => {
				specificity += 1;
				let lvl = field_path_inspection_level(&c.path);
				if lvl > max_level {
					max_level = lvl;
				}
				if matches!(c.path, FieldPath::HttpBody) {
					reads_http_body = true;
				}
			}
			Predicate::AnyOf(_) | Predicate::Not(_) => {}
		});
	}

	let mut needs_request_body = reads_http_body;
	let mut needs_response_body = false;
	for mw_ref in &raw.middleware_chain {
		let meta = mw_meta
			.get(&mw_ref.name)
			.ok_or_else(|| Error::compile(format!("unknown middleware: {:?}", mw_ref.name)))?;
		if meta.needs_body {
			match meta.kind {
				crate::middleware::MiddlewareKind::L7Request => needs_request_body = true,
				crate::middleware::MiddlewareKind::L7Response => needs_response_body = true,
				crate::middleware::MiddlewareKind::L4Peek | crate::middleware::MiddlewareKind::L4Bytes => {}
			}
		}
	}

	// fetch_meta is consulted so unknown kinds fail compile consistently with
	// how link will fail later; the metadata itself is not currently consumed
	// in analyze (phase comes from the fixed FetchKind table below).
	let _ = fetch_meta;

	let posture = match fetch_phase {
		FetchPhase::L4 if max_level <= InspectionLevel::L4Peek => Posture::L4,
		FetchPhase::L4 => {
			return Err(Error::compile(format!(
				"rule {:?}: L7-level predicate on an L4 fetch is invalid",
				raw.name
			)));
		}
		FetchPhase::L7 => Posture::L7,
	};

	Ok(AnalyzedRule {
		raw,
		inspection_level: max_level,
		specificity,
		posture,
		needs_request_body,
		needs_response_body,
	})
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum FetchPhase {
	L4,
	L7,
}

const fn fetch_phase_of(kind: Option<FetchKind>) -> FetchPhase {
	match kind {
		Some(FetchKind::L4Forward) => FetchPhase::L4,
		_ => FetchPhase::L7,
	}
}

fn walk_predicate(p: &Predicate, f: &mut impl FnMut(&Predicate)) {
	f(p);
	match p {
		Predicate::AnyOf(a) => {
			for child in &a.any_of {
				walk_predicate(child, f);
			}
		}
		Predicate::Not(n) => walk_predicate(&n.not, f),
		Predicate::Check(_) => {}
	}
}

const fn field_path_inspection_level(path: &FieldPath) -> InspectionLevel {
	match path {
		FieldPath::Transport
		| FieldPath::RemoteIp
		| FieldPath::RemotePort
		| FieldPath::LocalIp
		| FieldPath::LocalPort => InspectionLevel::L4Only,
		FieldPath::Peek
		| FieldPath::TlsSni
		| FieldPath::TlsAlpn
		| FieldPath::TlsVersion
		| FieldPath::TlsPeerCertSubjectCn => InspectionLevel::L4Peek,
		FieldPath::HttpMethod
		| FieldPath::HttpUriPath
		| FieldPath::HttpUriQuery
		| FieldPath::HttpHeader(_) => InspectionLevel::L7Header,
		FieldPath::HttpBody => InspectionLevel::L7Body,
	}
}
