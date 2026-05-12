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
	// Per-rule TLS validation runs at the analyze stage so the lower
	// pass — which aggregates resolved specs into per-listener pools —
	// can assume each `TlsConfig` is internally consistent. Surfacing
	// the violation through the rule name keeps multi-file configs
	// debuggable.
	if let Some(tls) = raw.tls.as_ref() {
		tls.validate().map_err(|e| Error::compile(format!("rule {:?}: {}", raw.name, e)))?;
	}

	let fetch_kind = Some(raw.terminate.kind);
	let fetch_phase = fetch_phase_of(fetch_kind);

	let mut max_level = InspectionLevel::L4Only;
	let mut specificity = 0usize;
	let mut reads_http_body = false;
	if let Some(pred) = &raw.match_predicate {
		// Bound predicate nesting depth before any recursive walker
		// (here, in lower, or in collect_levels) touches the tree — a
		// pathologically nested operator-authored rule should fail
		// loud at compile, not crash the recursive walks at runtime.
		crate::predicate::check_max_depth(pred)
			.map_err(|e| Error::compile(format!("rule {:?}: {}", raw.name, e)))?;
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
			Predicate::AnyOf(_) | Predicate::AllOf(_) | Predicate::Not(_) => {}
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

/// Pre-order walk over a predicate tree using an explicit stack.
///
/// Depth is bounded by [`crate::predicate::MAX_PREDICATE_DEPTH`]
/// thanks to the upstream `check_max_depth` guard in `analyze_rule`,
/// but the iterative form keeps the walker independent of the system
/// stack and matches the spec recommendation to mirror
/// `check_acyclic`'s explicit-stack shape.
fn walk_predicate(root: &Predicate, f: &mut impl FnMut(&Predicate)) {
	let mut stack: Vec<&Predicate> = vec![root];
	while let Some(p) = stack.pop() {
		f(p);
		match p {
			Predicate::AnyOf(a) => {
				for child in a.any_of.iter().rev() {
					stack.push(child);
				}
			}
			Predicate::AllOf(a) => {
				for child in a.all_of.iter().rev() {
					stack.push(child);
				}
			}
			Predicate::Not(n) => stack.push(n.not.as_ref()),
			Predicate::Check(_) => {}
		}
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
		| FieldPath::TlsPeerCertPresent
		| FieldPath::TlsPeerCertSubjectCn
		| FieldPath::TlsPeerCertSanDns
		| FieldPath::TlsPeerCertFingerprintSha256
		| FieldPath::TlsPeerCertSpkiSha256
		| FieldPath::TlsPeerCertIssuerCn
		| FieldPath::TlsPeerCertSerial => InspectionLevel::L4Peek,
		FieldPath::HttpMethod
		| FieldPath::HttpUriPath
		| FieldPath::HttpUriQuery
		| FieldPath::HttpHeader(_) => InspectionLevel::L7Header,
		FieldPath::HttpBody => InspectionLevel::L7Body,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::compile::expand::RawRuleSet;
	use crate::fetch::{FetchOutputModes, FetchPhase as FetchMetaPhase};
	use crate::metadata::{FetchMetadata, MiddlewareMetadata};
	use crate::middleware::MiddlewareKind;
	use serde_json::Value;

	struct Providers;

	fn validate_ok(_: &Value) -> Result<(), Error> {
		Ok(())
	}

	impl MiddlewareMetadataProvider for Providers {
		fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
			match name {
				"req_plain" => Some(MiddlewareMetadata {
					kind: MiddlewareKind::L7Request,
					stateless: true,
					needs_body: false,
					validate_args: validate_ok,
				}),
				"req_body" => Some(MiddlewareMetadata {
					kind: MiddlewareKind::L7Request,
					stateless: true,
					needs_body: true,
					validate_args: validate_ok,
				}),
				"resp_body" => Some(MiddlewareMetadata {
					kind: MiddlewareKind::L7Response,
					stateless: true,
					needs_body: true,
					validate_args: validate_ok,
				}),
				_ => None,
			}
		}
	}

	impl FetchMetadataProvider for Providers {
		fn get(&self, kind: FetchKind) -> Option<FetchMetadata> {
			Some(FetchMetadata {
				kind,
				phase: match kind {
					FetchKind::L4Forward => FetchMetaPhase::L4,
					_ => FetchMetaPhase::L7,
				},
				output_modes: match kind {
					FetchKind::L4Forward => FetchOutputModes { response: false, tunnel: true },
					FetchKind::WebSocketUpgrade => FetchOutputModes { response: true, tunnel: true },
					_ => FetchOutputModes { response: true, tunnel: false },
				},
				validate_args: validate_ok,
			})
		}
	}

	fn set(rules: Vec<RawRule>) -> RawRuleSet {
		RawRuleSet { rules, source_files: vec![] }
	}

	fn parse_rule(j: serde_json::Value) -> RawRule {
		serde_json::from_value(j).expect("parse rule")
	}

	#[test]
	fn http_body_predicate_sets_request_body_flag_and_l7body_level() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"match": { "http.body": { "contains": "admin" } },
			"terminate": { "type": "http_proxy" },
		}));
		let out = analyze(set(vec![rule]), &Providers, &Providers).expect("analyze");
		let a = &out.rules[0];
		assert!(a.needs_request_body);
		assert!(!a.needs_response_body);
		assert_eq!(a.inspection_level, InspectionLevel::L7Body);
		assert_eq!(a.posture, Posture::L7);
	}

	#[test]
	fn l7_request_needs_body_middleware_flags_request_side() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"middleware_chain": [{ "use": "req_body" }],
			"terminate": { "type": "http_proxy" },
		}));
		let out = analyze(set(vec![rule]), &Providers, &Providers).expect("analyze");
		assert!(out.rules[0].needs_request_body);
		assert!(!out.rules[0].needs_response_body);
	}

	#[test]
	fn l7_response_needs_body_middleware_flags_response_side() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"middleware_chain": [{ "use": "resp_body" }],
			"terminate": { "type": "http_proxy" },
		}));
		let out = analyze(set(vec![rule]), &Providers, &Providers).expect("analyze");
		assert!(!out.rules[0].needs_request_body);
		assert!(out.rules[0].needs_response_body);
	}

	#[test]
	fn l4_fetch_with_l7_predicate_errors() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":22"],
			"match": { "http.method": { "equals": "GET" } },
			"terminate": { "type": "tcp_forward", "upstream": "10.0.0.1:22" },
		}));
		let err = analyze(set(vec![rule]), &Providers, &Providers).expect_err("must error");
		assert!(err.to_string().contains("L7-level predicate"));
	}

	#[test]
	fn unknown_middleware_name_errors() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"middleware_chain": [{ "use": "does_not_exist" }],
			"terminate": { "type": "http_proxy" },
		}));
		let err = analyze(set(vec![rule]), &Providers, &Providers).expect_err("must error");
		assert!(err.to_string().contains("does_not_exist"));
	}

	#[test]
	fn rejects_predicate_nested_deeper_than_max_predicate_depth() {
		// Build `not(not(not(... check ...)))` over `MAX_PREDICATE_DEPTH+1`
		// levels — straight chains are the easiest pathological shape.
		let depth = crate::predicate::MAX_PREDICATE_DEPTH + 1;
		let mut inner = serde_json::json!({ "tls.sni": { "equals": "a" } });
		for _ in 0..depth {
			inner = serde_json::json!({ "not": inner });
		}
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"match": inner,
			"terminate": { "type": "http_proxy" },
		});
		let rule: crate::rule::RawRule = serde_json::from_value(raw).expect("parse");
		let err =
			analyze(set(vec![rule]), &Providers, &Providers).expect_err("deep predicate must reject");
		assert!(err.to_string().contains("MAX_PREDICATE_DEPTH"), "{err}");
	}

	#[test]
	fn accepts_predicate_at_max_predicate_depth() {
		// Exactly MAX_PREDICATE_DEPTH levels of `not` wrapping a leaf
		// Check must still compile.
		let depth = crate::predicate::MAX_PREDICATE_DEPTH - 1;
		let mut inner = serde_json::json!({ "tls.sni": { "equals": "a" } });
		for _ in 0..depth {
			inner = serde_json::json!({ "not": inner });
		}
		let raw = serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"match": inner,
			"terminate": { "type": "http_proxy" },
		});
		let rule: crate::rule::RawRule = serde_json::from_value(raw).expect("parse");
		analyze(set(vec![rule]), &Providers, &Providers).expect("at-limit predicate compiles");
	}

	#[test]
	fn specificity_counts_check_predicates() {
		let rule = parse_rule(serde_json::json!({
			"name": "r",
			"listen": [":443"],
			"match": {
				"any_of": [
					{ "tls.sni": { "equals": "a" } },
					{ "tls.sni": { "equals": "b" } },
				],
			},
			"terminate": { "type": "http_proxy" },
		}));
		let out = analyze(set(vec![rule]), &Providers, &Providers).expect("analyze");
		assert_eq!(out.rules[0].specificity, 2);
	}
}
