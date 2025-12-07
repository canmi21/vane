#!/usr/bin/env ruby

# Print debug info to Stderr
$stderr.puts "⚙ Starting execution..."

# Read all stdin safely
input_raw = STDIN.read
if input_raw.nil? || input_raw.strip.empty?
  $stderr.puts "✗ No input received on Stdin!"
  exit 1
end

$stderr.puts "⚙ Received Input: #{input_raw.strip}"

# Parse JSON manually for {"auth_token":"..."} structure
auth_token = input_raw[/\"auth_token\":\"([^\"]+)\"/, 1] || ""

# Business Logic
if auth_token == "secret123"
  $stderr.puts "✓ Auth success!"
  branch = "success"
  store = '{"user_role":"admin","verified":"true"}'
else
  $stderr.puts "✗ Auth failed. Token was: #{auth_token}"
  branch = "failure"
  store = '{"error_reason":"invalid_token"}'
end

# Output result to Stdout (compact JSON)
print "{\"branch\":\"#{branch}\",\"store\":#{store}}"
