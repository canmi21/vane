#!/usr/bin/env bash
# Warn about source files exceeding 500 lines.
# Skip marker: place "vane:no-line-limit" in the first 5 lines to opt out.
set -euo pipefail

LIMIT=500
count=0

# Binary/generated extensions to skip
skip_ext='png|jpg|jpeg|gif|ico|svg|webp|woff|woff2|ttf|eot|otf|wasm|lock|map|min\.js|min\.css'

while IFS= read -r file; do
  # skip deleted or binary/image files
  [[ -f "$file" ]] || continue
  if [[ "$file" =~ \.($skip_ext)$ ]]; then
    continue
  fi

  # skip non-text files (git binary detection)
  if ! git diff --no-index --quiet --numstat /dev/null "$file" 2>/dev/null; then
    if file --brief "$file" | grep -qiE 'binary|image|font|archive'; then
      continue
    fi
  fi

  # check opt-out marker in first 5 lines
  if head -n 5 "$file" 2>/dev/null | grep -q 'vane:no-line-limit'; then
    continue
  fi

  lines=$(wc -l < "$file" 2>/dev/null || echo 0)
  if (( lines > LIMIT )); then
    printf 'warning: %s (%d lines)\n' "$file" "$lines"
    ((count++)) || true
  fi
done < <(git ls-files)

if (( count > 0 )); then
  printf 'Found %d file(s) exceeding %d lines.\n' "$count" "$LIMIT"
fi

# Non-blocking — always exit 0
exit 0
