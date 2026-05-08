#!/usr/bin/env perl
# Consume the JSONL plan emitted by `publish-plan.pl` on stdin and
# act on each row in order. Two modes:
#
#   --mode=dry    `cargo publish --dry-run` per row. Crates whose
#                 workspace deps are all already on crates.io
#                 verify the package build; the rest go pack-only
#                 via `--no-verify` since their unpublished siblings
#                 aren't resolvable from the registry yet.
#
#   --mode=real   `cargo publish` per row. Polls the sparse index
#                 between dependents so each new version is visible
#                 before the next dependent's verify-build runs.
#                 Aborts on the first failure; rerun after fixing —
#                 already-published crates are detected by
#                 publish-plan.pl and emitted as `skip`.
#
# real mode runs `just gate` first (skip with `--skip-gate`) and
# requires `CARGO_REGISTRY_TOKEN` in the environment.

use strict;
use warnings;
use Getopt::Long;
use JSON::PP;

my $mode = '';
my $skip_gate = 0;
GetOptions(
	'mode=s'    => \$mode,
	'skip-gate' => \$skip_gate,
) or die "usage: publish-execute.pl --mode=dry|real [--skip-gate]\n";

unless ($mode eq 'dry' || $mode eq 'real') {
	die "publish-execute.pl: --mode=dry|real is required\n";
}

if ($mode eq 'real' && !defined $ENV{CARGO_REGISTRY_TOKEN}) {
	die "publish-execute.pl: --mode=real requires CARGO_REGISTRY_TOKEN\n";
}

# ─── pre-flight gate ──────────────────────────────────────────────
if ($mode eq 'real') {
	if ($skip_gate) {
		print STDERR "publish-execute.pl: gate skipped via --skip-gate\n";
	} else {
		print "publish-execute.pl: running just gate ...\n";
		system('just', 'gate') == 0
			or die "publish-execute.pl: just gate failed\n";
	}
}

# ─── slurp plan from stdin ────────────────────────────────────────
my @plan;
while (my $line = <STDIN>) {
	chomp $line;
	next if $line eq '';
	push @plan, decode_json($line);
}
@plan or die "publish-execute.pl: empty plan on stdin\n";

# ─── crates.io sparse-index helpers (mirror plan.pl) ──────────────
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
	$body =~ s/\n(\d+)\s*$//s or return 0;
	my $code = $1;
	return 0 if $code eq '404';
	return 0 if $code ne '200';
	return $body =~ /"vers":"\Q$version\E"/ ? 1 : 0;
}

# Capped exponential backoff: 2s start, 10s max, 60s deadline.
# Cargo 1.66+ already waits for publish by default; this is the
# explicit gate before the next dependent's verify-build.
sub wait_for_index {
	my ($name, $version) = @_;
	my ($interval, $elapsed, $deadline) = (2, 0, 60);
	while ($elapsed < $deadline) {
		return 1 if version_published($name, $version);
		sleep $interval;
		$elapsed += $interval;
		my $bump = int($interval / 2) || 1;
		$interval = $interval + $bump > 10 ? 10 : $interval + $bump;
	}
	die "publish-execute.pl: timeout waiting for $name\@$version on sparse index\n";
}

# ─── execute ──────────────────────────────────────────────────────
# `available`: crates whose latest version is on the registry from
# this script's perspective. Initially every crate marked `skip`,
# plus each newly-published crate after `wait_for_index` returns.
my %available = map { $_->{name} => 1 } grep { $_->{action} eq 'skip' } @plan;

for my $row (@plan) {
	my ($action, $name, $version, $manifest, $deps) =
		@$row{qw(action name version manifest deps)};

	if ($action eq 'skip') {
		printf "  [skip]    %s %s\n", $name, $version;
		next;
	}

	my $all_avail = !grep { !$available{$_} } @$deps;

	if ($mode eq 'dry') {
		if ($all_avail) {
			printf "  [dry]     %s %s (verify)\n", $name, $version;
			system('cargo', 'publish', '--dry-run', '--allow-dirty',
				'--manifest-path', $manifest) == 0
				or die "publish-execute.pl: dry-run failed for $name\n";
		} else {
			printf "  [dry]     %s %s (no-verify, unpublished sibling)\n",
				$name, $version;
			system('cargo', 'publish', '--dry-run', '--no-verify',
				'--allow-dirty', '--manifest-path', $manifest) == 0
				or die "publish-execute.pl: dry-run failed for $name\n";
		}
	} else {
		$all_avail or die
			"publish-execute.pl: $name has unpublished workspace deps; topo bug\n";
		printf "  [publish] %s %s\n", $name, $version;
		system('cargo', 'publish', '--manifest-path', $manifest) == 0
			or die "publish-execute.pl: publish failed for $name\n";
		wait_for_index($name, $version);
		$available{$name} = 1;
	}
}

print "\npublish-execute.pl: ${mode}-run complete\n";
