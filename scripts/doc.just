# rustdoc + doctests; `-D warnings` makes broken intra-doc links fail.
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
	cargo test --doc --workspace
