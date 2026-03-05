#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

printf '\n==> Checking markdown links\n'

broken=0
checked=0

while IFS= read -r file; do
  dir="$(dirname "$file")"
  in_fence=false

  while IFS= read -r line; do
    # Track fenced code blocks to skip links inside them.
    if [[ "$line" =~ ^\`\`\` ]]; then
      if $in_fence; then
        in_fence=false
      else
        in_fence=true
      fi
      continue
    fi
    $in_fence && continue

    # Extract markdown link targets [text](path) — loop handles multiple per line.
    remainder="$line"
    link_re='\]\(([^)]+)\)'
    while [[ "$remainder" =~ $link_re ]]; do
      raw="${BASH_REMATCH[1]}"
      remainder="${remainder#*"${BASH_REMATCH[0]}"}"

      # Skip absolute URLs, mailto, and pure anchors.
      [[ "$raw" =~ ^https?:// ]] && continue
      [[ "$raw" =~ ^mailto: ]] && continue
      [[ "$raw" =~ ^# ]] && continue

      # Strip query parameters.
      path="${raw%%\?*}"

      # Separate anchor from path.
      anchor=""
      if [[ "$path" == *"#"* ]]; then
        anchor="${path#*#}"
        path="${path%%#*}"
      fi

      [[ -z "$path" ]] && continue

      # Resolve relative to source file's directory.
      resolved="$dir/$path"

      checked=$((checked + 1))

      if [[ ! -e "$resolved" ]]; then
        printf '  BROKEN: %s -> %s\n' "${file#"$ROOT"/}" "$raw"
        broken=$((broken + 1))
        continue
      fi

      # Verify heading anchor exists in the target file.
      if [[ -n "$anchor" && -f "$resolved" ]]; then
        slug_pattern="${anchor//-/[- ]}"
        if ! grep -qiE "^#{1,6}[[:space:]]+${slug_pattern}[[:space:]]*$" "$resolved" 2>/dev/null; then
          printf '  BROKEN ANCHOR: %s -> %s (heading #%s not found)\n' \
            "${file#"$ROOT"/}" "$path" "$anchor"
          broken=$((broken + 1))
        fi
      fi
    done
  done < "$file"
done < <(
  find "$ROOT" -name '*.md' -type f \
    -not -path '*/node_modules/*' \
    -not -path '*/target/*' \
    -not -path '*/dist/*' \
    -not -path '*/.git/*' \
  | sort
)

printf '\n==> Links checked: %d, broken: %d\n' "$checked" "$broken"

if [[ "$broken" -gt 0 ]]; then
  exit 1
fi
