#!/usr/bin/env python3
# examples/plugins/exec/test_python_template.py

import sys
import json


# Safely read all content from Stdin
def read_all_stdin():
    lines = []
    for line in sys.stdin:
        lines.append(line.rstrip("\n"))
    return "\n".join(lines)


# --- Main Logic ---

# Print debug info to Stderr (Vane logs this)
print("⚙ Starting execution...", file=sys.stderr)

# Read ResolvedInputs from Vane
input_raw = read_all_stdin()
if not input_raw:
    print("✗ No input received on Stdin!", file=sys.stderr)
    sys.exit(1)

print(f"⚙ Received Input: {input_raw}", file=sys.stderr)

# Parse JSON
try:
    inputs = json.loads(input_raw)
except json.JSONDecodeError as e:
    print(f"✗ Invalid JSON: {e}", file=sys.stderr)
    sys.exit(1)

# Business Logic
# Assume Vane passes an argument "auth_token"
# If token is "secret123", return success branch and write user info to KV
output = {}

if inputs.get("auth_token") == "secret123":
    print("✓ Auth success!", file=sys.stderr)
    output = {"branch": "success", "store": {"user_role": "admin", "verified": "true"}}
else:
    print(f"✗ Auth failed. Token was: {inputs.get('auth_token')}", file=sys.stderr)
    output = {"branch": "failure", "store": {"error_reason": "invalid_token"}}

# Output result to Stdout (Must be compact JSON)
output_json = json.dumps(output, separators=(",", ":"))
sys.stdout.write(output_json)