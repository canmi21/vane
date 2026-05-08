#!/usr/bin/env perl
# Build the workspace's `vane` CLI binary and write its path to
# nextest's per-run env file so daemon mgmt tests can spawn the CLI
# without paying a runtime `cargo build` (and the cargo lock
# contention that would imply across parallel test processes).
#
# Driven by `.config/nextest.toml`'s `build-vane-cli` setup script.
# Stand-alone runs work too: point `NEXTEST_ENV` at a scratch file
# (`NEXTEST_ENV=/tmp/env perl scripts/build-vane-bin.pl`) and inspect
# the resulting `VANE_BIN=` line.
#
# Path extraction goes through `cargo build --message-format=json`
# rather than hard-coding `target/debug/vane` so the script keeps
# working under `CARGO_TARGET_DIR` overrides, `--target <triple>`
# cross-compilation, and `--release` invocations. cargo emits one
# JSON object per line; we keep the one whose `target.name` is
# `vane`, `target.kind` contains `bin`, and `executable` is set,
# then echo its `executable` field.

use strict;
use warnings;
use JSON::PP;

defined $ENV{NEXTEST_ENV}
	or die "build-vane-bin.pl: NEXTEST_ENV is unset; this script is meant to "
		. "run as a nextest setup script\n";

open my $cargo, '-|',
	'cargo', 'build', '-p', 'vane', '--bin', 'vane',
		'--message-format=json', '--quiet'
	or die "build-vane-bin.pl: cargo build failed to start: $!\n";

my $bin;
while (my $line = <$cargo>) {
	my $msg = eval { decode_json($line) };
	next unless ref($msg) eq 'HASH';
	next unless ($msg->{reason} // '') eq 'compiler-artifact';
	next unless ($msg->{target}{name} // '') eq 'vane';
	next unless grep { $_ eq 'bin' } @{ $msg->{target}{kind} // [] };
	next unless $msg->{executable};
	$bin = $msg->{executable};
	last;
}
close $cargo;
$? == 0 or die "build-vane-bin.pl: cargo build exited non-zero\n";

defined $bin
	or die "build-vane-bin.pl: could not extract vane binary path from cargo build output\n";
-x $bin
	or die "build-vane-bin.pl: extracted path $bin is not executable\n";

open my $env, '>>', $ENV{NEXTEST_ENV}
	or die "build-vane-bin.pl: cannot append to $ENV{NEXTEST_ENV}: $!\n";
print $env "VANE_BIN=$bin\n";
close $env;
