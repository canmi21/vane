#!/usr/bin/env lua

-- A simple HTTP server using LuaSocket to mock a Vane middleware plugin.
-- Requires: luarocks install luasocket

local socket = require "socket"
local json_lib_status, json = pcall(require, "json") -- Try to load json lib safely

-- Simple JSON fallback if external lib is missing (for basic test structure)
if not json_lib_status then
	json = {
		decode = function(str)
			-- Extremely naive parser for {"auth_token":"..."}
			local token = str:match '"auth_token"%s*:%s*"(.-)"'
			return { auth_token = token }
		end,
		encode = function(tbl)
			-- Naive encoder for specific response structure
			if tbl.status == "success" then
				-- Handle nested structure manually for the fallback
				return string.format('{"status":"success","data":{"branch":"%s","store":{"user":"admin"}}}', tbl.data.branch)
			else
				return string.format('{"status":"error","message":"%s"}', tbl.message)
			end
		end,
	}
end

local port = arg[1] or "0" -- 0 lets OS pick port
local server = assert(socket.bind("*", tonumber(port)))
local ip, actual_port = server:getsockname()

-- Print port to stdout so test runner knows where to connect
print(actual_port)
io.stdout:flush()

while true do
	local client = server:accept()
	if client then
		client:settimeout(10)

		local line, err = client:receive()
		if not err then
			-- Read headers to find Content-Length
			local content_length = 0
			while line and line ~= "" do
				local len_str = line:match "Content%-Length:%s*(%d+)"
				if len_str then
					-- Fix: ensure result is not nil and explicitly handled as number
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
						data = { branch = "success", store = { user = "admin" } },
					}
				else
					response_data = {
						status = "success", -- API call succeeded, logic returned failure branch
						data = { branch = "failure", store = { error = "invalid_token" } },
					}
				end

				local response_body = json.encode(response_data)

				client:send "HTTP/1.1 200 OK\r\n"
				client:send "Content-Type: application/json\r\n"
				client:send("Content-Length: " .. #response_body .. "\r\n")
				client:send "\r\n"
				client:send(response_body)
			end
		end
		client:close()
	end
end
