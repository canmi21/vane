#!/usr/bin/env lua

local cjson = require "cjson"

-- Safely read all content from Stdin
local function read_all_stdin()
    local lines = {}
    while true do
        local line = io.read()
        if line == nil then break end
        table.insert(lines, line)
    end
    return table.concat(lines, "\n")
end

-- --- Main Logic ---

-- Print debug info to Stderr (Vane logs this)
io.stderr:write("⚙ Starting execution...\n")

-- Read ResolvedInputs from Vane
local input_raw = read_all_stdin()
if input_raw == "" or input_raw == nil then
    io.stderr:write("✗ No input received on Stdin!\n")
    os.exit(1)
end

io.stderr:write("⚙ Received Input: " .. input_raw .. "\n")

-- 3. Parse JSON
local status, inputs = pcall(cjson.decode, input_raw)
if not status then
    io.stderr:write("✗ Invalid JSON: " .. inputs .. "\n")
    os.exit(1)
end

-- Business Logic
-- Assume Vane passes an argument "auth_token"
-- If token is "secret123", return success branch and write user info to KV
local output = {}

if inputs["auth_token"] == "secret123" then
    io.stderr:write("✓ Auth success!\n")
    output = {
        branch = "success",
        store = {
            ["user_role"] = "admin",
            ["verified"] = "true"
        }
    }
else
    io.stderr:write("✗ Auth failed. Token was: " .. tostring(inputs["auth_token"]) .. "\n")
    output = {
        branch = "failure",
        store = {
            ["error_reason"] = "invalid_token"
        }
    }
end

-- Output result to Stdout (Must be compact JSON)
local output_json = cjson.encode(output)
io.write(output_json)

-- End