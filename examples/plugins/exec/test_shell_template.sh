#!/usr/bin/env bash
# examples/plugins/exec/test_shell_template.sh

# Print debug info to Stderr
echo "⚙ Starting execution..." >&2

# Read all stdin safely using cat, which is robust for buffered input
input_raw=$(cat)

if [ -z "$input_raw" ]; then
    echo "✗ No input received on Stdin!" >&2
    exit 1
fi

echo "⚙ Received Input: $input_raw" >&2

# Parse JSON manually for {"auth_token":"..."} structure
# We use 'sed' instead of 'grep -P' because macOS (BSD) grep does not support Perl regex.
# Logic: Find "auth_token":"VALUE", capture VALUE, and print it.
auth_token=$(echo "$input_raw" | sed -n 's/.*"auth_token":"\([^"]*\)".*/\1/p')

# Business Logic
if [ "$auth_token" = "secret123" ]; then
    echo "✓ Auth success!" >&2
    branch="success"
    store='{"user_role":"admin","verified":"true"}'
else
    echo "✗ Auth failed. Token was: $auth_token" >&2
    branch="failure"
    store='{"error_reason":"invalid_token"}'
fi

# Output result to Stdout (compact JSON)
printf '{"branch":"%s","store":%s}' "$branch" "$store"