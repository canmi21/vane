#!/usr/bin/env bash

# Print debug info to Stderr
echo "⚙ Starting execution..." >&2

# Read all stdin safely
input_raw=""
while IFS= read -r line; do
    input_raw="${input_raw}${line}\n"
done

if [ -z "$input_raw" ]; then
    echo "✗ No input received on Stdin!" >&2
    exit 1
fi

# Remove trailing newline
input_raw=$(echo -e "$input_raw" | sed '$ s/\n$//')

echo "⚙ Received Input: $input_raw" >&2

# Parse JSON manually for {"auth_token":"..."} structure
auth_token=$(echo "$input_raw" | grep -oP '"auth_token":"\K[^"]+')

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
