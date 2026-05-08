//! Generates wasm component fixtures for tests in `vane-wasm` and the
//! daemon's wasm-loader / wasm-e2e suites. Gated behind the
//! `wasm-fixtures` feature so testutil consumers that don't run wasm
//! tests don't pay for `wat` / `wit-component` / `wit-parser` in
//! their build dep graph.
//!
//! Outputs land in `OUT_DIR` (per Cargo policy — a build script must
//! never modify anything in the source tree). The absolute paths are
//! exposed to consumers through `cargo:rustc-env`, then re-exported
//! by `src/wasm_fixture.rs` via `env!`. See that module for the
//! consumer-facing API.
//!
//! The WIT source lives in `crates/wasm/wit/` (the wasm crate's own
//! `wasmtime::component::bindgen!` reads from the same tree). We
//! load it via a workspace-relative path and stay in lockstep with
//! `vane-wasm`'s interface definitions.

#[cfg(not(feature = "wasm-fixtures"))]
fn main() {
	// Feature off: no fixture generation, no `rerun-if-changed` triggers.
	// The `wasm_fixture` module is also gated off, so the `env!()` calls
	// inside it never run.
}

#[cfg(feature = "wasm-fixtures")]
fn main() {
	use std::path::PathBuf;

	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed=../wasm/wit");

	let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
	let wit_dir = manifest_dir.join("../wasm/wit");
	let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));

	let metadata_out = out_dir.join("metadata_fixture.wasm");
	let mismatch_out = out_dir.join("mismatch_fixture.wasm");

	// Full fixture: exports registry + handler-l4-peek; metadata claims probe/l4-peek.
	wasm_fixtures::generate(
		&wit_dir,
		r"
package vane-wasm:fixture@0.1.0;
world fixture-plugin {
    export vane:plugin/registry@0.1.0;
    export vane:plugin/handler-l4-peek@0.1.0;
}
",
		"fixture-plugin",
		wasm_fixtures::FIXTURE_WAT,
		&metadata_out,
	);

	// Mismatch fixture: exports registry only, but metadata claims an l4-peek
	// export. Used by `load_component` rejection tests.
	wasm_fixtures::generate(
		&wit_dir,
		r"
package vane-wasm:mismatch@0.1.0;
world mismatch-plugin {
    export vane:plugin/registry@0.1.0;
}
",
		"mismatch-plugin",
		wasm_fixtures::MISMATCH_WAT,
		&mismatch_out,
	);

	println!("cargo:rustc-env=VANE_TESTUTIL_WASM_METADATA_FIXTURE={}", metadata_out.display());
	println!("cargo:rustc-env=VANE_TESTUTIL_WASM_MISMATCH_FIXTURE={}", mismatch_out.display());
}

#[cfg(feature = "wasm-fixtures")]
mod wasm_fixtures {
	use std::path::Path;

	use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
	use wit_parser::Resolve;

	pub(super) fn generate(
		wit_dir: &Path,
		wit_src: &str,
		world_name: &str,
		core_wat: &str,
		out: &Path,
	) {
		let mut resolve = Resolve::default();
		resolve.push_dir(wit_dir).expect("failed to parse WIT dir");

		let pkg = resolve.push_str("inline.wit", wit_src).expect("failed to parse inline WIT");
		let world = resolve.select_world(&[pkg], Some(world_name)).expect("failed to find world");

		let mut core_bytes = wat::parse_str(core_wat).expect("failed to parse WAT");
		embed_component_metadata(&mut core_bytes, &resolve, world, StringEncoding::UTF8)
			.expect("failed to embed component metadata");

		let component = ComponentEncoder::default()
			.module(&core_bytes)
			.expect("failed to set module")
			.encode()
			.expect("failed to encode component");

		std::fs::create_dir_all(out.parent().unwrap()).expect("failed to create OUT_DIR parent");
		std::fs::write(out, &component).expect("failed to write fixture");
	}

	// Mismatch fixture: claims an l4-peek export named "probe" in metadata but
	// the WAT only exports the registry interface — handler-l4-peek is
	// intentionally absent. Used to test that load_component rejects the
	// kind/handler mismatch.
	//
	// Memory layout:
	//   0-7:   "mismatch" (8 bytes)
	//   8-12:  "0.1.0"   (5 bytes, version and abi-version)
	//   13-17: "probe"   (5 bytes, export name)
	//   18-19: zero pad  (2 bytes, align to 20)
	//   20-43: middleware-export struct (24 bytes):
	//     [20] name.ptr=13  [24] name.len=5
	//     [28] kind=0(l4-peek) [29] stateless=1 [30..31] pad
	//     [32] inspects.ptr=0  [36] inspects.len=0
	//     [40] needs-streaming-body=0 [41-43] pad
	pub(super) const MISMATCH_WAT: &str = r#"(module
  (memory (export "cm32p2_memory") 1)
  (global $heap (mut i32) (i32.const 256))
  (data (i32.const 0)
    "mismatch"
    "0.1.0"
    "probe"
    "\00\00"
    "\0d\00\00\00\05\00\00\00\00\01\00\00\00\00\00\00\00\00\00\00\00\00\00"
  )
  (func $alloc (export "cm32p2_realloc") (param i32 i32 i32 i32) (result i32)
    (local $r i32)
    (local.set $r
      (i32.and
        (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get 2))
      )
    )
    (global.set $heap (i32.add (local.get $r) (local.get 3)))
    (local.get $r)
  )
  (func (export "cm32p2|vane:plugin/registry@0.1|get-metadata") (result i32)
    (local $r i32)
    (local.set $r (call $alloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 32)))
    (i32.store (local.get $r) (i32.const 0))
    (i32.store offset=4 (local.get $r) (i32.const 8))
    (i32.store offset=8 (local.get $r) (i32.const 8))
    (i32.store offset=12 (local.get $r) (i32.const 5))
    (i32.store offset=16 (local.get $r) (i32.const 8))
    (i32.store offset=20 (local.get $r) (i32.const 5))
    (i32.store offset=24 (local.get $r) (i32.const 20))
    (i32.store offset=28 (local.get $r) (i32.const 1))
    (local.get $r)
  )
)"#;

	// Full fixture (memory layout documented in the WAT data segment offsets).
	//
	//   0-6:   "fixture"  (7 bytes)
	//   7-11:  "0.1.0"    (5 bytes, used for both version and abi-version)
	//   12-16: "probe"    (5 bytes, the single export name)
	//   17-19: zero pad   (align to 20)
	//   20-43: middleware-export struct (24 bytes):
	//     [20] name.ptr=12  [24] name.len=5
	//     [28] kind=0(l4-peek) [29] stateless=1 [30] needs-body=0 [31] pad
	//     [32] inspects.ptr=0  [36] inspects.len=0
	//     [40] needs-streaming-body=0 [41-43] pad
	//
	// Heap starts at 256; cm32p2_realloc is a bump allocator. `get-metadata`
	// returns a pointer to a guest-allocated metadata struct (canonical ABI for
	// >MAX_FLAT_RESULTS results); `handle` mirrors the same shape and is left
	// unreachable beyond the smoke-test it covers.
	pub(super) const FIXTURE_WAT: &str = r#"(module
  (memory (export "cm32p2_memory") 1)
  (global $heap (mut i32) (i32.const 256))
  (data (i32.const 0)
    "fixture"
    "0.1.0"
    "probe"
    "\00\00\00"
    "\0c\00\00\00\05\00\00\00\00\01\00\00\00\00\00\00\00\00\00\00\00\00\00\00"
  )
  (func $alloc (export "cm32p2_realloc") (param i32 i32 i32 i32) (result i32)
    (local $r i32)
    (local.set $r
      (i32.and
        (i32.add (global.get $heap) (i32.sub (local.get 2) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get 2))
      )
    )
    (global.set $heap (i32.add (local.get $r) (local.get 3)))
    (local.get $r)
  )
  (func (export "cm32p2|vane:plugin/registry@0.1|get-metadata") (result i32)
    (local $r i32)
    (local.set $r (call $alloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 32)))
    (i32.store (local.get $r) (i32.const 0))
    (i32.store offset=4 (local.get $r) (i32.const 7))
    (i32.store offset=8 (local.get $r) (i32.const 7))
    (i32.store offset=12 (local.get $r) (i32.const 5))
    (i32.store offset=16 (local.get $r) (i32.const 7))
    (i32.store offset=20 (local.get $r) (i32.const 5))
    (i32.store offset=24 (local.get $r) (i32.const 20))
    (i32.store offset=28 (local.get $r) (i32.const 1))
    (local.get $r)
  )
  (func (export "cm32p2|vane:plugin/handler-l4-peek@0.1|handle")
    (param i32 i32 i32 i32 i32 i32) (result i32)
    (local $r i32) (local $decision i32)
    (if (i32.eqz (i32.load (i32.const 64)))
      (then
        (i32.store (i32.const 64) (i32.const 42))
        (local.set $decision (i32.const 0)))
      (else
        (local.set $decision (i32.const 1))))
    (local.set $r (call $alloc (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 32)))
    (i32.store          (local.get $r) (i32.const 0))
    (i32.store offset=4  (local.get $r) (local.get $decision))
    (i32.store offset=8  (local.get $r) (i32.const 0))
    (i32.store offset=12 (local.get $r) (i32.const 0))
    (i32.store offset=16 (local.get $r) (i32.const 0))
    (i32.store offset=20 (local.get $r) (i32.const 0))
    (i32.store offset=24 (local.get $r) (i32.const 0))
    (i32.store offset=28 (local.get $r) (i32.const 0))
    (local.get $r)
  )
)"#;
}
