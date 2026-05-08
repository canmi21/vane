#!/usr/bin/env perl
# Emit the workspace publish plan: every publishable crate, in
# topological order, marked `skip` (already on crates.io at this
# version) or `publish` (new version that needs uploading).
#
# Output:
#   - tty stdout    → human-readable table
#   - piped stdout  → newline-delimited JSON, one row per crate, the
#                     stable contract for `publish-execute.pl`
#
# Each JSONL row carries everything `publish-execute.pl` needs to run
# without re-querying anything:
#   { "action": "skip"|"publish",
#     "name":   "...",
#     "version": "...",
#     "manifest": "/abs/path/Cargo.toml",
#     "deps":   ["other-publishable-crate", ...] }
#
# `deps` is filtered to other publishable members only, and dev-deps
# are excluded since `cargo publish` strips them on package.

use strict;
use warnings;
use Getopt::Long;
use JSON::PP;

my $only = '';
GetOptions('only=s' => \$only) or die "usage: publish-plan.pl [--only=CRATE]\n";

# ─── workspace metadata ───────────────────────────────────────────
my $meta_json = qx(cargo metadata --format-version 1 --no-deps);
$? == 0 or die "publish-plan.pl: cargo metadata failed\n";
my $meta = decode_json($meta_json);

my %members;   # name -> { version, manifest }
for my $p (@{$meta->{packages}}) {
	# `publish = false` shows up as an empty array in cargo metadata.
	next if ref($p->{publish}) eq 'ARRAY' && !@{$p->{publish}};
	$members{$p->{name}} = {
		version  => $p->{version},
		manifest => $p->{manifest_path},
	};
}

# ─── intra-workspace deps (non-dev only) ──────────────────────────
my %deps;   # name -> [other publishable members it depends on]
for my $p (@{$meta->{packages}}) {
	next unless exists $members{$p->{name}};
	my @ws_deps;
	for my $d (@{$p->{dependencies}}) {
		next if defined $d->{kind} && $d->{kind} eq 'dev';
		push @ws_deps, $d->{name} if exists $members{$d->{name}};
	}
	# de-duplicate while preserving order
	my %seen;
	$deps{$p->{name}} = [grep { !$seen{$_}++ } @ws_deps];
}

# ─── topological sort (Kahn, alpha-stable) ────────────────────────
my %remaining_deps = map { $_ => { map { $_ => 1 } @{$deps{$_}} } } keys %members;
my @order;
while (%remaining_deps) {
	my @ready = sort grep { !%{$remaining_deps{$_}} } keys %remaining_deps;
	@ready or die "publish-plan.pl: dependency cycle among: "
		. join(", ", sort keys %remaining_deps) . "\n";
	push @order, @ready;
	delete @remaining_deps{@ready};
	for my $name (keys %remaining_deps) {
		delete $remaining_deps{$name}{$_} for @ready;
	}
}

# ─── crates.io sparse-index existence check ───────────────────────
sub crate_to_index_path {
	my ($name) = @_;
	my $n = length $name;
	return "1/$name"                              if $n == 1;
	return "2/$name"                              if $n == 2;
	return "3/" . substr($name, 0, 1) . "/$name"  if $n == 3;
	return substr($name, 0, 2) . "/" . substr($name, 2, 2) . "/$name";
}

sub version_published {
	my ($name, $version) = @_;
	my $url  = "https://index.crates.io/" . crate_to_index_path($name);
	my $body = qx(curl -sS --max-time 15 --retry 2 --retry-delay 1 \\
		-w '\\n%{http_code}' "$url" 2>/dev/null);
	$body =~ s/\n(\d+)\s*$//s or return 2;
	my $code = $1;
	return 1 if $code eq '404';
	return 2 if $code ne '200';
	return $body =~ /"vers":"\Q$version\E"/ ? 0 : 1;
}

# ─── build the plan ───────────────────────────────────────────────
my @plan;
for my $name (@order) {
	next if $only ne '' && $name ne $only;

	my $m = $members{$name};
	my $rc = version_published($name, $m->{version});
	if ($rc == 2) {
		die "publish-plan.pl: sparse-index lookup failed for $name\n";
	}
	push @plan, {
		action   => $rc == 0 ? 'skip' : 'publish',
		name     => $name,
		version  => $m->{version},
		manifest => $m->{manifest},
		deps     => $deps{$name},
	};
}

if ($only ne '' && !@plan) {
	die "publish-plan.pl: --only=$only did not match any publishable crate\n";
}

# ─── output ───────────────────────────────────────────────────────
if (-t STDOUT) {
	printf "\n  PLAN\n";
	printf "  %-9s %-30s %s\n", "ACTION", "CRATE", "VERSION";
	printf "  %s\n", "-" x 52;
	for my $row (@plan) {
		printf "  %-9s %-30s %s\n",
			"[$row->{action}]", $row->{name}, $row->{version};
	}
	print "\n";
} else {
	my $json = JSON::PP->new->canonical(1);
	print $json->encode($_), "\n" for @plan;
}
