-- integration/tests/l7/cgi-bin/sample_bin.lua

-- Read Environment Variables
local method = os.getenv "REQUEST_METHOD" or "(null)"
local content_length = os.getenv "CONTENT_LENGTH"
local query = os.getenv "QUERY_STRING" or "(null)"

-- Read Body
local body = ""
if content_length then
	local len = tonumber(content_length)
	if len and len > 0 then
		body = io.read(len) or ""
	end
end

-- Output Headers
print "Status: 200 OK"
print "Content-Type: text/plain"
print "X-CGI-Test: Vane-Lua-Script"
print "" -- End of Headers

-- Output Body
print "CGI Output (Lua):"
print("Method: " .. method)
print("Query: " .. query)
print("Body Len: " .. (content_length or "0"))
print("Body Content: " .. body)
