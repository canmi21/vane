#!/bin/sh
# Build the workspace's `vane` CLI binary and write its path to
# nextest's per-run env file so daemon mgmt tests can spawn the
# CLI without paying a runtime `cargo build` (and the cargo lock
# contention that would imply across parallel test processes).
#
# Driven by `.config/nextest.toml`'s `build-vane-cli` setup script.
# Stand-alone runs work too: point `NEXTEST_ENV` at a scratch file
# (`NEXTEST_ENV=/tmp/env scripts/build-vane-bin.sh`) and inspect
# the resulting `VANE_BIN=` line.
#
# Path extraction goes through `cargo build --message-format=json`
# rather than hard-coding `target/debug/vane` so the script keeps
# working under `CARGO_TARGET_DIR` overrides, `--target <triple>`
# cross-compilation, and `--release` invocations. cargo emits one
# JSON object per line; we keep the one whose `target.name` is
# `vane`, `target.kind` contains `bin`, and `executable` is set,
# then echo its `executable` field.

set -eu

if [ -z "${NEXTEST_ENV:-}" ]; then
	echo "build-vane-bin.sh: NEXTEST_ENV is unset; this script is meant to run as a nextest setup script" >&2
	exit 1
fi

output=$(cargo build -p vane --bin vane --message-format=json --quiet)

bin=$(printf '%s\n' "$output" | perl -MJSON::PP -ne '
	my $msg = eval { JSON::PP->new->decode($_) } or next;
	next unless ref($msg) eq "HASH";
	next unless ($msg->{reason} // "") eq "compiler-artifact";
	next unless ($msg->{target}{name} // "") eq "vane";
	next unless grep { $_ eq "bin" } @{ $msg->{target}{kind} // [] };
	next unless $msg->{executable};
	print $msg->{executable};
	last;
')

if [ -z "$bin" ]; then
	echo "build-vane-bin.sh: could not extract vane binary path from cargo build output" >&2
	exit 1
fi

if [ ! -x "$bin" ]; then
	echo "build-vane-bin.sh: extracted path $bin is not executable" >&2
	exit 1
fi

echo "VANE_BIN=$bin" >> "$NEXTEST_ENV"
