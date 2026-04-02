// Run with: cargo test -p vane-panel --test export_types
//
// Exports panel API types to TypeScript bindings for the Svelte frontend.
// Output: src/panel/web/src/types/bindings.ts

#[allow(clippy::unwrap_used)]
#[test]
fn export_typescript_bindings() {
	use specta::Types;
	use specta_typescript::Typescript;
	use vane_panel::{
		GetConfigOutput, ListConnectionsOutput, SystemInfoOutput, UpdateConfigInput, UpdateConfigOutput,
	};

	let types = Types::default()
		.register::<ListConnectionsOutput>()
		.register::<SystemInfoOutput>()
		.register::<GetConfigOutput>()
		.register::<UpdateConfigInput>()
		.register::<UpdateConfigOutput>();

	let resolved = specta_serde::apply_phases(types).unwrap();

	let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web/src/types");
	std::fs::create_dir_all(&out_dir).unwrap();

	let out_path = out_dir.join("bindings.ts");
	Typescript::default().export_to(&out_path, &resolved).unwrap();

	println!("exported bindings to {}", out_path.display());
}
