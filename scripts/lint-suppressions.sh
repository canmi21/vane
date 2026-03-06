#!/usr/bin/env bash
# Audit all lint-suppression markers across Rust and Go source files.
# Manual tool — not wired into the default lint pipeline.
set -euo pipefail

total=0

search() {
  local label="$1" pattern="$2"
  shift 2
  local results
  results=$(git grep -n "$pattern" -- "$@" 2>/dev/null || true)
  if [[ -n "$results" ]]; then
    local n
    n=$(echo "$results" | wc -l | tr -d ' ')
    printf '\n--- %s (%d) ---\n' "$label" "$n"
    echo "$results"
    ((total += n)) || true
  fi
}

# Rust
search '#[allow(' '#\[allow(' '*.rs'
search '#[cfg_attr(..allow' 'cfg_attr.*allow' '*.rs'

# Go
search "//nolint" "//nolint" '*.go'

# Line-length opt-out
search "vane:no-line-limit" "vane:no-line-limit" '*.sh' '*.rs' '*.go' '*.ts'

printf '\n========================================\n'
printf 'Total suppressions: %d\n' "$total"
