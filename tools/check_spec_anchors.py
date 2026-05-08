#!/usr/bin/env python3
"""Verify every `spec/<path>.md § _Section_` reference in workspace
source resolves to a real heading in that spec file.

Each `§ _Section_` reference is paired with the closest preceding
`spec/<path>.md` token on the same line. If none is on that line, the
checker looks back up to 30 lines for the most recent `spec/...md`
mention (so doc-block headers carry forward to continuation lines).
Heading match is exact — substrings, abbreviations, or near-matches
do not pass.

Exit code: 0 when every ref resolves; 1 when at least one is broken
(printed grouped by target).

Run from the repo root: `python3 tools/check_spec_anchors.py`."""

import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC_DIR = os.path.join(REPO, "spec")
CRATES_DIR = os.path.join(REPO, "crates")

spec_headings = {}
for root, _, files in os.walk(SPEC_DIR):
    for f in files:
        if not f.endswith(".md"):
            continue
        full = os.path.join(root, f)
        rel = os.path.relpath(full, REPO)
        with open(full) as fh:
            text = fh.read()
        headings = set()
        for line in text.splitlines():
            m = re.match(r"^#{1,6}\s+(.+?)\s*$", line)
            if m:
                headings.add(m.group(1).strip())
        spec_headings[rel] = headings

# Match either a spec/path.md token or a § _section_ marker.
TOKEN_RE = re.compile(
    r"(spec/[A-Za-z0-9_/-]+\.md)"
    r"|"
    r"§\s+_([^_\n][^_\n]*?)_(?=[\s.,;:)\]/`*\n]|$)"
)

broken = []
total = 0

for root, _, files in os.walk(CRATES_DIR):
    if "/target/" in root:
        continue
    for f in files:
        if not f.endswith(".rs"):
            continue
        full = os.path.join(root, f)
        with open(full) as fh:
            lines = fh.readlines()
        # Carry the most recent spec path across lines (within a 30-line
        # window) so trailing `§ _Section_` continuation lines pick up
        # the file from a recent doc-block header.
        carry_path = None
        carry_line = -100
        for i, line in enumerate(lines):
            current_path = carry_path if (i - carry_line) <= 30 else None
            for tok in TOKEN_RE.finditer(line):
                if tok.group(1):
                    current_path = tok.group(1)
                    carry_path = current_path
                    carry_line = i
                else:
                    sec = tok.group(2).strip().rstrip(".")
                    total += 1
                    if current_path is None:
                        broken.append((f"{os.path.relpath(full, REPO)}:{i+1}", "<no-file>", sec))
                        continue
                    if current_path not in spec_headings:
                        broken.append((f"{os.path.relpath(full, REPO)}:{i+1}", current_path, f"<missing-file> {sec}"))
                        continue
                    if sec not in spec_headings[current_path]:
                        broken.append((f"{os.path.relpath(full, REPO)}:{i+1}", current_path, sec))

print(f"Total scanned: {total}")
print(f"Broken: {len(broken)}")
if broken:
    print()
    from collections import defaultdict
    groups = defaultdict(list)
    for site, sf, sec in broken:
        groups[(sf, sec)].append(site)
    for (sf, sec), sites in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        print(f"  {sf} § _{sec}_  ({len(sites)} site{'s' if len(sites)!=1 else ''})")
        for s in sites:
            print(f"    {s}")
    sys.exit(1)
sys.exit(0)
