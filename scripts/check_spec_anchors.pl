#!/usr/bin/env perl
# Verify every `spec/<path>.md § _Section_` reference in workspace
# source resolves to a real heading in that spec file.
#
# Each `§ _Section_` is paired with the closest preceding
# `spec/<path>.md` token on the same line; if none on that line, the
# checker looks back up to 30 lines for the most recent mention so
# doc-block headers carry forward to continuation lines. Heading
# match is exact — substrings, abbreviations, or near-matches do
# not pass.
#
# Run from the repo root: `perl scripts/check_spec_anchors.pl`.
# Exit 0 when every reference resolves; exit 1 with a grouped
# report otherwise.

use strict;
use warnings;
use File::Find;
use File::Spec;

my $root = `git rev-parse --show-toplevel`;
chomp $root;
chdir $root or die "chdir $root: $!";

# Build heading index: { spec_path => { heading => 1 } }
my %headings;
find({
    wanted => sub {
        return unless /\.md\z/ && -f $File::Find::name;
        my $path = File::Spec->abs2rel($File::Find::name, $root);
        open my $fh, '<', $File::Find::name or return;
        while (my $line = <$fh>) {
            if ($line =~ /^\#{1,6}\s+(.+?)\s*$/) {
                $headings{$path}{$1} = 1;
            }
        }
    },
    no_chdir => 1,
}, 'spec');

# Token regex: alternation of `spec/path.md` or `§ _Section_`. The
# closing underscore must be followed by whitespace, punctuation, or
# end of line so identifiers like `forward_client_ip` aren't eaten.
my $token = qr{
    (spec/[A-Za-z0-9_/-]+\.md)
    |
    § \s+ _ ([^_\n][^_\n]*?) _ (?=[\s.,;:)\]/`*\n]|\z)
}x;

my $total = 0;
my %broken;  # "$file\t$section" => [sites]

my @sources;
find({
    wanted => sub {
        return unless /\.rs\z/ && -f $File::Find::name;
        return if $File::Find::name =~ m{/target/};
        push @sources, File::Spec->abs2rel($File::Find::name, $root);
    },
    no_chdir => 1,
}, 'crates');

for my $src (@sources) {
    open my $fh, '<', $src or next;
    my @lines = <$fh>;
    my ($carry_path, $carry_line) = (undef, -100);
    for my $i (0 .. $#lines) {
        my $line = $lines[$i];
        my $current = ($i - $carry_line <= 30) ? $carry_path : undef;
        while ($line =~ /$token/g) {
            if (defined $1) {
                $current = $1;
                $carry_path = $current;
                $carry_line = $i;
            } else {
                my $sec = $2;
                $sec =~ s/\.+\z//;
                $total++;
                my $key;
                if (!defined $current) {
                    $key = "<no-file>\t$sec";
                } elsif (!exists $headings{$current}) {
                    $key = "<missing-file> $current\t$sec";
                } elsif (!$headings{$current}{$sec}) {
                    $key = "$current\t$sec";
                } else {
                    next;
                }
                push @{ $broken{$key} }, "$src:" . ($i + 1);
            }
        }
    }
}

my $broken_count = 0;
$broken_count += scalar @$_ for values %broken;

print "Total scanned: $total\n";
print "Broken: $broken_count\n";

if ($broken_count) {
    print "\n";
    for my $key (sort { @{ $broken{$b} } <=> @{ $broken{$a} } || $a cmp $b } keys %broken) {
        my ($sf, $sec) = split /\t/, $key, 2;
        my @sites = @{ $broken{$key} };
        my $plural = @sites == 1 ? '' : 's';
        printf "  %s § _%s_  (%d site%s)\n", $sf, $sec, scalar(@sites), $plural;
        print "    $_\n" for @sites;
    }
    exit 1;
}
exit 0;
