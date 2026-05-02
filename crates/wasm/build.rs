use std::path::{Path, PathBuf};

use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::Resolve;

fn main() {
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed=wit/");

	let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
	let fixtures = manifest_dir.join("fixtures");

	// Full fixture: exports registry + handler-l4-peek, metadata claims probe/l4-peek.
	generate_fixture(
		&manifest_dir,
		r"
package vane-wasm:fixture@0.1.0;
world fixture-plugin {
    export vane:plugin/registry@0.1.0;
    export vane:plugin/handler-l4-peek@0.1.0;
}
",
		"fixture-plugin",
		FIXTURE_WAT,
		&fixtures.join("metadata_fixture.wasm"),
	);

	// Mismatch fixture: exports registry only, but metadata claims an l4-peek export.
	// Used to test handler-kind validation rejection in load_component.
	generate_fixture(
		&manifest_dir,
		r"
package vane-wasm:mismatch@0.1.0;
world mismatch-plugin {
    export vane:plugin/registry@0.1.0;
}
",
		"mismatch-plugin",
		MISMATCH_WAT,
		&fixtures.join("mismatch_fixture.wasm"),
	);
}

fn generate_fixture(
	manifest_dir: &Path,
	wit_src: &str,
	world_name: &str,
	core_wat: &str,
	out: &Path,
) {
	let mut resolve = Resolve::default();
	resolve.push_dir(manifest_dir.join("wit")).expect("failed to parse WIT dir");

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

	std::fs::create_dir_all(out.parent().unwrap()).expect("failed to create fixtures dir");
	std::fs::write(out, &component).expect("failed to write fixture");
}

// Core WAT implementing the fixture plugin with static canonical-ABI data.
//
// Memory layout (address 0 onwards, little-endian):
//   0-6:   "fixture"  (7 bytes)
//   7-11:  "0.1.0"    (5 bytes, used for both version and abi-version)
//   12-16: "probe"    (5 bytes, the single export name)
//   17-19: zero pad   (align to 20)
//   20-43: middleware-export struct (24 bytes, canonical ABI layout for align=4):
//     [20] name.ptr=12  [24] name.len=5
//     [28] kind=0(l4-peek) [29] stateless=1 [30] needs-body=0 [31] pad
//     [32] inspects.ptr=0  [36] inspects.len=0
//     [40] needs-streaming-body=0 [41-43] pad
//
// Heap starts at 256; cm32p2_realloc is a bump allocator.
//
// get-metadata() -> (i32):
//   Returns complex type (metadata, 8 flat fields > MAX_FLAT_RESULTS=1), so the
//   canonical ABI encodes it as: the GUEST allocates via cm32p2_realloc, writes
//   the struct, and RETURNS the pointer as the single i32 result.
//
// handle() -> (i32):
//   Same pattern: result<l4-peek-decision, plugin-error> has 8 flat fields.
//   The handle body is unreachable (we only test load/metadata in Step 1).
//
// Export names use Standard32 mangling: cm32p2|<iface@compat-ver>|<func>
// compat-ver for 0.1.0 is "0.1" per PackageName::version_compat_track_string.
// Mismatch fixture: claims an l4-peek export named "probe" in metadata but the WAT
// only exports the registry interface — handler-l4-peek is intentionally absent.
// Used to test that load_component rejects the kind/handler mismatch.
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
const MISMATCH_WAT: &str = r#"(module
  (memory (export "cm32p2_memory") 1)
  (global $heap (mut i32) (i32.const 256))
  (data (i32.const 0)
    "mismatch"
    "0.1.0"
    "probe"
    "\00\00"
    "\0d\00\00\00\05\00\00\00\00\01\00\00\00\00\00\00\00\00\00\00\00\00\00\00"
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

const FIXTURE_WAT: &str = r#"(module
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
    unreachable
  )
)"#;
