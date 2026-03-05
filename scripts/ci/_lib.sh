#!/usr/bin/env bash
# Shared helpers for CI runner scripts.
# Usage: source "$(dirname "$0")/ci/_lib.sh"

# require_cmd <cmd> [install-hint]
# Exits with a clear message if a required tool is missing.
require_cmd() {
  local cmd="$1"
  local hint="${2:-}"
  if ! command -v "$cmd" &>/dev/null; then
    printf '==> ERROR: %s not found.' "$cmd"
    [[ -n "$hint" ]] && printf ' Install: %s' "$hint"
    printf '\n'
    exit 1
  fi
}

# run_parallel <label1> <script1> [<label2> <script2> ...]
# Runs scripts in parallel, waits for all, reports failures by label.
run_parallel() {
  local pids=()
  local labels=()

  while [[ $# -ge 2 ]]; do
    labels+=("$1")
    eval "$2" &
    pids+=($!)
    shift 2
  done

  local failed=()
  for i in "${!pids[@]}"; do
    if ! wait "${pids[$i]}"; then
      failed+=("${labels[$i]}")
    fi
  done

  if [[ ${#failed[@]} -gt 0 ]]; then
    printf '\n==> FAILED: %s\n' "${failed[*]}"
    exit 1
  fi
}
