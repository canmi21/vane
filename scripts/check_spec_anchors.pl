#!/usr/bin/env perl
# Verify every `spec/<path>.md § _Section_` reference in workspace
# source resolves to a real heading in that spec file.
#
# Per-comment-block, position-aware:
#
# - Contiguous `//`, `///`, `//!` lines are folded into one logical
#   block (prefix stripped, joined with spaces) so wrapped anchors
#   like `§ _Update\n//   model_` are seen as one logical token.
# - Each `§ _Section_` is paired with the closest preceding
#   `spec/<path>.md` token in the same block; if none, the most
#   recent mention from the previous 30 source lines carries.
# - Slash continuations `§ _A_ / _B_ / _C_` treat _B_ and _C_ as
#   sibling sections under the same spec file as _A_.
# - Adjacent identical `§ _X_ § _X_` is reported as a duplicate
#   regression (the Round 3 bulk sweep collapsed two distinct
#   references into the same anchor in three places).
# - Heading match is exact — no substring fallback, no near-match.
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

# Token regex: spec path | primary § _X_ | slash continuation / _Y_.
# The closing underscore must be followed by whitespace, punctuation,
# or end of input so identifiers like `forward_client_ip` aren't eaten.
my $token = qr{
    (spec/[A-Za-z0-9_/-]+\.md)                                   # 1: spec path
    |
    § \s+ _ ([^_\n][^_\n]*?) _ (?=[\s.,;:)\]/`*]|\z)             # 2: primary section
    |
    / \s+ _ ([^_\n][^_\n]*?) _ (?=[\s.,;:)\]/`*]|\z)             # 3: continuation
}x;

my $total = 0;
my %broken;  # "kind\tfile\tsection" => [sites]

my @sources;
find({
    wanted => sub {
        return unless /\.rs\z/ && -f $File::Find::name;
        return if $File::Find::name =~ m{/target/};
        push @sources, File::Spec->abs2rel($File::Find::name, $root);
    },
    no_chdir => 1,
}, 'crates');

for my $src (sort @sources) {
    open my $fh, '<', $src or next;
    my @lines = <$fh>;

    # Group contiguous comment lines into blocks. Each block carries
    # the joined text plus a parallel array mapping the joined-text
    # offset back to the source line number for reporting.
    my @blocks;
    my (@cur_text, @cur_offsets);
    for my $i (0 .. $#lines) {
        if ($lines[$i] =~ m{^\s*//[!/]?[ \t]?(.*?)\s*$}) {
            my $body = $1;
            push @cur_text, $body;
            # Map every character of $body (plus the joining space) to
            # the line index. The trailing space joiner uses the same
            # line so `§\n//   _name_` resolves to the line carrying §.
            push @cur_offsets, ($i) x (length($body) + 1);
        } else {
            if (@cur_text) {
                push @blocks, {
                    text => join(' ', @cur_text),
                    offset_to_line => [@cur_offsets],
                };
                (@cur_text, @cur_offsets) = ();
            }
        }
    }
    if (@cur_text) {
        push @blocks, {
            text => join(' ', @cur_text),
            offset_to_line => [@cur_offsets],
        };
    }

    my ($carry_path, $carry_line) = (undef, -100);
    for my $block (@blocks) {
        my $text = $block->{text};
        my $o2l = $block->{offset_to_line};
        my $first_line = $o2l->[0] // 0;
        my $current = ($first_line - $carry_line <= 30) ? $carry_path : undef;
        my $last_section;

        while ($text =~ /$token/g) {
            my $match_pos = $-[0];
            my $line_num = ($o2l->[$match_pos] // $first_line) + 1;
            if (defined $1) {
                $current = $1;
                $carry_path = $current;
                $carry_line = $o2l->[$match_pos] // $first_line;
                $last_section = undef;
            } elsif (defined $2 || defined $3) {
                my $sec = defined $2 ? $2 : $3;
                $sec =~ s/\s+/ /g;
                $sec =~ s/\.+\z//;
                $total++;
                if (defined $2 && defined $last_section && $last_section eq $sec) {
                    push @{ $broken{"DUP\t" . ($current // '<no-file>') . "\t$sec"} },
                        "$src:$line_num";
                }
                $last_section = $sec if defined $2;
                if (!defined $current) {
                    push @{ $broken{"NOFILE\t<no-file>\t$sec"} }, "$src:$line_num";
                    next;
                }
                if (!exists $headings{$current}) {
                    push @{ $broken{"MISSFILE\t$current\t$sec"} }, "$src:$line_num";
                    next;
                }
                if (!$headings{$current}{$sec}) {
                    push @{ $broken{"MISSHEAD\t$current\t$sec"} }, "$src:$line_num";
                    next;
                }
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
    for my $key (sort {
        @{ $broken{$b} } <=> @{ $broken{$a} } || $a cmp $b
    } keys %broken) {
        my ($kind, $sf, $sec) = split /\t/, $key, 3;
        my @sites = @{ $broken{$key} };
        my $plural = @sites == 1 ? '' : 's';
        my $tag = {
            DUP      => 'duplicate',
            NOFILE   => 'no spec file in scope',
            MISSFILE => 'spec file not found',
            MISSHEAD => 'heading not in spec file',
        }->{$kind} // $kind;
        printf "  [%s] %s § _%s_  (%d site%s)\n",
            $tag, $sf, $sec, scalar(@sites), $plural;
        print "    $_\n" for @sites;
    }
    exit 1;
}
exit 0;
