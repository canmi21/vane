#!/usr/bin/env perl
# Sync the `version = "..."` field of every path-based entry in the
# root Cargo.toml's `[workspace.dependencies]` section against its
# real source of truth:
#
#   - `crates/lib/<name>`   → that crate's own `[package].version`.
#   - `crates/<name>`       → typically `version.workspace = true`,
#                             so it falls back to the root's
#                             `[workspace.package].version`.
#
# Two modes:
#
#   --check   exit non-zero on any drift, list which entries are out
#             of sync. Used by lefthook pre-commit when no auto-fix
#             is wanted, and by CI.
#
#   --write   rewrite the root Cargo.toml in place, bringing every
#             stale `version =` into line. Used by lefthook (with
#             `stage_fixed: true`) and as the first step of
#             scripts/publish.sh.
#
# The substitution is line-anchored: it only touches the `version =
# "..."` field on each `[workspace.dependencies]` line, leaving every
# other field (path, default-features, features) intact.
#
# Stdlib only — perl is the same dependency every other repo helper
# script already takes (see scripts/check_spec_anchors.pl).

use strict;
use warnings;

my $mode = shift @ARGV // '';
unless ($mode eq '--check' || $mode eq '--write') {
	die "usage: scripts/sync-workspace-deps.pl --check | --write\n";
}

# Locate the workspace root. The script is meant to run from the
# repo root, but resolve relative to argv[0] so `just` invocations
# from anywhere still work.
my $root_cargo;
for my $candidate ('Cargo.toml', "$0/../../Cargo.toml") {
	if (-f $candidate) { $root_cargo = $candidate; last; }
}
defined $root_cargo or die "cannot locate workspace Cargo.toml\n";

my @lines = do {
	open my $fh, '<', $root_cargo or die "read $root_cargo: $!";
	<$fh>;
};

# Pull `[workspace.package].version` for vane-* crates that inherit it.
my $ws_pkg_version;
{
	my $in_pkg = 0;
	for my $line (@lines) {
		if    ($line =~ /^\[workspace\.package\]/) { $in_pkg = 1; next; }
		elsif ($line =~ /^\[/)                     { $in_pkg = 0; next; }
		if ($in_pkg && $line =~ /^version\s*=\s*"([^"]+)"/) {
			$ws_pkg_version = $1;
			last;
		}
	}
}
defined $ws_pkg_version
	or die "no [workspace.package].version found in $root_cargo\n";

# Resolve a crate's effective version from its own Cargo.toml.
sub crate_version {
	my ($path) = @_;
	my $cargo = "$path/Cargo.toml";
	-f $cargo or return undef;
	open my $fh, '<', $cargo or die "read $cargo: $!";
	my $in_pkg = 0;
	while (my $line = <$fh>) {
		if    ($line =~ /^\[package\]/) { $in_pkg = 1; next; }
		elsif ($line =~ /^\[/)          { $in_pkg = 0; next; }
		next unless $in_pkg;
		return $1                if $line =~ /^version\s*=\s*"([^"]+)"/;
		return $ws_pkg_version   if $line =~ /^version\.workspace\s*=\s*true/;
	}
	return undef;
}

# Walk [workspace.dependencies], compare, optionally rewrite.
my $in_deps = 0;
my @drift;
for (my $i = 0; $i < @lines; $i++) {
	my $line = $lines[$i];

	if    ($line =~ /^\[workspace\.dependencies\]/) { $in_deps = 1; next; }
	elsif ($line =~ /^\[/)                          { $in_deps = 0; next; }
	next unless $in_deps;

	# Inline-table entry that carries a `path = "..."` field.
	next unless $line =~ /^\s*([A-Za-z0-9_-]+)\s*=\s*\{[^}]*path\s*=\s*"([^"]+)"/;
	my ($name, $path) = ($1, $2);

	# Entry without a `version = "..."` field (e.g. `vane-testutil`,
	# which is `publish = false`) — nothing to sync.
	next unless $line =~ /version\s*=\s*"([^"]+)"/;
	my $current = $1;

	my $expected = crate_version($path);
	defined $expected or next;

	next if $current eq $expected;

	push @drift, {
		name     => $name,
		current  => $current,
		expected => $expected,
		index    => $i,
	};
}

if (!@drift) {
	print "all workspace dep versions in sync\n";
	exit 0;
}

if ($mode eq '--check') {
	print STDERR "workspace dep version drift:\n";
	for my $d (@drift) {
		printf STDERR "  %-30s %s -> %s\n", $d->{name}, $d->{current}, $d->{expected};
	}
	print STDERR "\nrun: just sync-deps   (or sh scripts/sync-workspace-deps.pl --write)\n";
	exit 1;
}

# --write: rewrite root Cargo.toml.
for my $d (@drift) {
	my $line = $lines[$d->{index}];
	$line =~ s/(version\s*=\s*")\Q$d->{current}\E(")/$1$d->{expected}$2/
		or die "internal: failed to rewrite version for $d->{name}\n";
	$lines[$d->{index}] = $line;
}

open my $out, '>', $root_cargo or die "write $root_cargo: $!";
print $out @lines;
close $out;

print "synced workspace dep versions:\n";
for my $d (@drift) {
	printf "  %-30s %s -> %s\n", $d->{name}, $d->{current}, $d->{expected};
}
