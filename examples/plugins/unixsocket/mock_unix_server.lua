#!/usr/bin/env lua

-- A simple HTTP server over Unix Domain Socket to mock a Vane middleware plugin.
-- Requires: luasocket (and specifically socket.unix)

local socket = require "socket"
local unix = require "socket.unix"

-- Simple JSON fallback (same as the HTTP example)
local json = {
	decode = function(str)
		local token = str:match '"auth_token"%s*:%s*"(.-)"'
		return { auth_token = token }
	end,
	encode = function(tbl)
		if tbl.status == "success" then
			return string.format('{"status":"success","data":{"branch":"%s","store":{"user":"admin"}}}', tbl.data.branch)
		else
			return string.format('{"status":"error","message":"%s"}', tbl.message)
		end
	end,
}

local path = arg[1]
if not path then
	print "Usage: lua mock_unix_server.lua <socket_path>"
	os.exit(1)
end

-- Ensure clean state
os.remove(path)

local server = assert(unix())
assert(server:bind(path))
assert(server:listen())

print("Listening on " .. path)
io.stdout:flush()

while true do
	local client = server:accept()
	if client then
		client:settimeout(5)

		-- Vane sends raw HTTP over UDS
		local line, err = client:receive()
		if not err then
			-- Read headers to find Content-Length
			local content_length = 0
			while line and line ~= "" do
				local len_str = line:match "Content%-Length:%s*(%d+)"
				if len_str then
					content_length = tonumber(len_str) or 0
				end
				line, err = client:receive()
			end

			-- Read Body
			local body, err_body = client:receive(content_length)

			if body then
				local input = json.decode(body)

				-- Logic: Check auth_token
				local response_data
				if input and input.auth_token == "secret123" then
					response_data = {
						status = "success",
						data = { branch = "success" },
					}
				else
					response_data = {
						status = "success",
						data = { branch = "failure" },
					}
				end

				local response_body = json.encode(response_data)

				-- Send HTTP Response
				client:send "HTTP/1.1 200 OK\r\n"
				client:send "Content-Type: application/json\r\n"
				client:send("Content-Length: " .. #response_body .. "\r\n")
				client:send "Connection: close\r\n" -- Vane expects close or reads len
				client:send "\r\n"
				client:send(response_body)
			end
		end
		client:close()
	end
end
