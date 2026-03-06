-- integration/tests/l7/cgi-bin/redirect_login.lua
-- Simulates a login CGI that returns 302 redirect with session cookie

-- Read Environment Variables
local method = os.getenv("REQUEST_METHOD") or "GET"
local content_length = os.getenv("CONTENT_LENGTH") or "0"

-- Read POST Body (simulated form data)
local body = ""
local len = tonumber(content_length)
if len and len > 0 then
	body = io.read(len) or ""
end

-- Debug: Log what we received (to stderr, visible in Vane debug logs)
io.stderr:write(string.format("DEBUG: method=%s, content_length=%s, body_len=%d\n",
	method, content_length, #body))

-- Simulate Authentication (check if body contains credentials)
local authenticated = false
if method == "POST" and #body > 0 then
	-- Check if body contains username and password fields
	if body:find("username=") and body:find("password=") then
		authenticated = true
		io.stderr:write("DEBUG: Authentication SUCCESS\n")
	else
		io.stderr:write("DEBUG: Authentication FAILED - invalid body format\n")
	end
else
	io.stderr:write(string.format("DEBUG: Authentication FAILED - method=%s, body_len=%d\n", method, #body))
end

if authenticated then
	-- Output 302 Redirect with Session Cookie
	print("Status: 302 Found")
	print("Set-Cookie: session_id=test_session_12345; path=/; HttpOnly")
	print("Location: /dashboard")
	print("Content-Type: text/html")
	print("") -- End of Headers
	-- Optional: 302 can have empty body or minimal body
	print("<html><body>Redirecting...</body></html>")
else
	-- Return 401 Unauthorized for missing/invalid credentials
	print("Status: 401 Unauthorized")
	print("Content-Type: text/plain")
	print("") -- End of Headers
	print("Authentication required")
end
