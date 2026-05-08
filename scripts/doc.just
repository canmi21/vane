# Rustdoc + doctest pipeline. `RUSTDOCFLAGS=-D warnings` turns
# broken intra-doc links and other rustdoc warnings into errors;
# `cargo test --doc` compiles every `//!` / `///` example and runs
# the ones that are not `no_run`.

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
	cargo test --doc --workspace
