//! Integration tests for the config loader.
//!
//! Two surfaces under test:
//!
//! 1. The directory-walking + dotenvy precedence behaviour of
//!    [`vane_core::config::load`].
//! 2. The full pipeline `load → merge → expand → analyze → lower →
//!    validate` for a realistic deployment-shaped config tree.
//!
//! Tests that mutate process env are marked `#[serial]` (via
//! `serial_test`) so they don't race each other. Rust 1.95 marks
//! `std::env::set_var` / `remove_var` `unsafe`; the unsafe is sound
//! under serial execution because no other test-thread is reading the
//! env concurrently. The workspace lint `unsafe_code = "deny"` is
//! relaxed here for that reason — there is no race-safe alternative
//! for testing the actual dotenvy/OS-env precedence chain end-to-end
//! (unit-level coverage uses the `EnvReader` test seam instead).

#![allow(unsafe_code)] // std::env::set_var / remove_var are unsafe in 2024 edition; serial test isolation makes it sound.

use std::fs;

use serial_test::serial;
use vane_core::compile::compile;
use vane_core::config::load;
use vane_core::fetch::{FetchKind, FetchOutputModes, FetchPhase};
use vane_core::metadata::{
	FetchMetadata, FetchMetadataProvider, MiddlewareMetadata, MiddlewareMetadataProvider,
};
use vane_core::middleware::MiddlewareKind;

/// The set of `VANE_*` keys that integration tests touch. Cleared before
/// every serial test so prior values from the host environment, dotenvy
/// loads, or sibling tests don't leak across.
const TOUCHED_KEYS: &[&str] = &[
	"VANE_WASM_DIR",
	"VANE_LOG_LEVEL",
	"VANE_BIND_IPV4",
	"VANE_BIND_IPV6",
	"VANE_SEC_MAX_HEADER_BYTES",
	"VANE_SEC_MAX_HEADERS_COUNT",
	"VANE_SEC_HEADER_TIMEOUT",
	"VANE_SEC_MAX_CONN_PER_IP",
	"VANE_MGMT_UNIX",
	"VANE_MGMT_HTTP_PORT",
	"VANE_MGMT_HTTP_PUBLIC",
	"VANE_MGMT_HTTP_TOKEN",
];

fn clear_touched_env() {
	for key in TOUCHED_KEYS {
		// SAFETY: serial_test ensures no concurrent env reads.
		unsafe {
			std::env::remove_var(key);
		}
	}
}

/// Minimal `MetadataProvider` for the e2e pipeline test — registers the
/// middleware and fetch shapes that `reverse_proxy` synthesises.
struct Providers;

fn validate_ok(_: &serde_json::Value) -> Result<(), vane_core::Error> {
	Ok(())
}

impl MiddlewareMetadataProvider for Providers {
	fn get(&self, name: &str) -> Option<MiddlewareMetadata> {
		match name {
			"forward_client_ip" => Some(MiddlewareMetadata {
				kind: MiddlewareKind::L7Request,
				stateless: true,
				needs_body: false,
				validate_args: validate_ok,
			}),
			"rate_limit" => Some(MiddlewareMetadata {
				kind: MiddlewareKind::L7Request,
				stateless: false,
				needs_body: false,
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
				FetchKind::L4Forward => FetchPhase::L4,
				_ => FetchPhase::L7,
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

#[test]
#[serial]
fn load_with_dotenv_populates_env_when_os_unset() {
	clear_touched_env();
	let dir = tempfile::tempdir().expect("tempdir");
	fs::write(dir.path().join(".env"), "VANE_BIND_IPV4=0\nVANE_LOG_LEVEL=debug\n").unwrap();
	fs::create_dir(dir.path().join("rules")).unwrap();

	let loaded = load(dir.path()).expect("load");
	assert!(!loaded.env.bind_ipv4, ".env value populated bind_ipv4=false");
	assert_eq!(loaded.env.log_level, "debug");
}

#[test]
#[serial]
fn load_os_env_wins_over_dotenv() {
	// dotenvy::from_path does not override pre-existing keys — operators
	// who set env via systemd EnvironmentFile= always win.
	clear_touched_env();
	// SAFETY: serial_test ensures no concurrent env reads.
	unsafe {
		std::env::set_var("VANE_BIND_IPV4", "1");
	}
	let dir = tempfile::tempdir().expect("tempdir");
	fs::write(dir.path().join(".env"), "VANE_BIND_IPV4=0\n").unwrap();
	fs::create_dir(dir.path().join("rules")).unwrap();

	let loaded = load(dir.path()).expect("load");
	assert!(loaded.env.bind_ipv4, "OS env (=1) must win over .env (=0)");
	clear_touched_env();
}

#[test]
#[serial]
fn load_without_dotenv_returns_defaults() {
	clear_touched_env();
	let dir = tempfile::tempdir().expect("tempdir");
	fs::create_dir(dir.path().join("rules")).unwrap();

	let loaded = load(dir.path()).expect("load");
	assert!(loaded.env.bind_ipv4, "default true when neither .env nor OS env set");
	assert_eq!(loaded.env.log_level, "info");
	assert_eq!(loaded.env.sec_max_header_bytes, 65_536);
}

#[test]
#[serial]
fn load_returns_files_from_rules_subdir() {
	clear_touched_env();
	let dir = tempfile::tempdir().expect("tempdir");
	fs::create_dir(dir.path().join("rules")).unwrap();
	fs::write(
		dir.path().join("rules").join("00-test.json"),
		r#"{"order": 0, "rules": [{
			"name": "r",
			"listen": [":7900"],
			"terminate": { "type": "http_proxy", "upstream": "127.0.0.1:8080" }
		}]}"#,
	)
	.unwrap();

	let loaded = load(dir.path()).expect("load");
	assert_eq!(loaded.files.len(), 1);
	assert_eq!(loaded.files[0].path.file_name().and_then(|s| s.to_str()), Some("00-test.json"));
}

#[test]
#[serial]
fn load_missing_rules_dir_errors() {
	clear_touched_env();
	let dir = tempfile::tempdir().expect("tempdir");
	// no rules/ subdir created.
	let err = load(dir.path()).expect_err("missing rules dir errors");
	assert!(err.to_string().contains("rules directory not found"), "{err}");
}

#[test]
#[serial]
fn load_pipeline_compiles_end_to_end() {
	// Realistic config tree: a `reverse_proxy` preset rule under rules/,
	// a `.env` setting log level, no config.json (deferred). Loaded files
	// thread through the full compile pipeline and produce a usable
	// SymbolicFlowGraph.
	clear_touched_env();
	let dir = tempfile::tempdir().expect("tempdir");
	fs::write(dir.path().join(".env"), "VANE_LOG_LEVEL=debug\n").unwrap();
	fs::create_dir(dir.path().join("rules")).unwrap();
	fs::write(
		dir.path().join("rules").join("10-api.json"),
		r#"{
			"order": 10,
			"rules": [{
				"preset": "reverse_proxy",
				"name": "api",
				"listen": [":7901"],
				"args": { "upstream": "127.0.0.1:8080", "websocket": false }
			}]
		}"#,
	)
	.unwrap();

	let loaded = load(dir.path()).expect("load");
	assert_eq!(loaded.env.log_level, "debug");
	assert_eq!(loaded.files.len(), 1);

	let graph = compile(loaded.files, &Providers, &Providers).expect("pipeline compiles");
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpProxy),
		"main rule emits HttpProxy"
	);
	assert!(
		graph.fetches.iter().any(|f| f.kind == FetchKind::HttpSynthesize),
		"ws-disable gate emits HttpSynthesize",
	);
	assert!(
		graph.nodes.iter().any(|n| matches!(n, vane_core::Node::Upgrade { .. })),
		"L7 listener Upgrade"
	);
}
