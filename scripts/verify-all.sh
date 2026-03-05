#!/usr/bin/env bash
# Single-command verification: format, lint, build, test.
# Usage: bash scripts/verify-all.sh
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
source "$DIR/ci/_lib.sh"

require_cmd cargo "https://rustup.rs"
require_cmd bun   "https://bun.sh"
require_cmd go    "https://go.dev/dl"

just fmt-check

run_parallel "lint" "just lint-ox lint-go lint-links" "clippy" "just lint-clippy" "test-rs" "just test-rs"

just inst

run_parallel "test-integration" "just test-integration"

printf '\n==> All checks passed.\n'
