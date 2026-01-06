/* integration/tests/l7/suite.go */
package l7

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

type TestEntry struct {
	Name string
	Desc string
	Run  func(context.Context, *env.Sandbox) error
}

func GetTests() []TestEntry {
	return []TestEntry{
		// Basic & Matrix
		{Name: "l7_test_https_proxy_basic", Desc: "Basic HTTPS Proxy (H1->H1)", Run: TestHttpsProxy},
		{Name: "l7_test_h1_to_h1", Desc: "Matrix: H1 Client -> H1 Upstream", Run: TestH1toH1},
		{Name: "l7_test_h1_to_h2", Desc: "Matrix: H1 Client -> H2 Upstream", Run: TestH1toH2},
		{Name: "l7_test_h1_to_h3", Desc: "Matrix: H1 Client -> H3 Upstream", Run: TestH1toH3},
		{Name: "l7_test_h2_to_h1", Desc: "Matrix: H2 Client -> H1 Upstream", Run: TestH2toH1},
		{Name: "l7_test_h2_to_h2", Desc: "Matrix: H2 Client -> H2 Upstream", Run: TestH2toH2},
		{Name: "l7_test_h2_to_h3", Desc: "Matrix: H2 Client -> H3 Upstream", Run: TestH2toH3},
		{Name: "l7_test_h3_to_h1", Desc: "Matrix: H3 Client -> H1 Upstream", Run: TestH3toH1},
		{Name: "l7_test_h3_to_h2", Desc: "Matrix: H3 Client -> H2 Upstream", Run: TestH3toH2},
		{Name: "l7_test_h3_to_h3", Desc: "Matrix: H3 Client -> H3 Upstream", Run: TestH3toH3},

		// Advanced Content Tests
		{Name: "l7_test_adv_status_h2", Desc: "Advanced: Status Codes over H2/TLS", Run: TestStatusH2},
		{Name: "l7_test_adv_status_h3", Desc: "Advanced: Status Codes over H3/QUIC", Run: TestStatusH3},
		{Name: "l7_test_adv_body_h2", Desc: "Advanced: Body Types over H2/TLS", Run: TestBodyH2},
		{Name: "l7_test_adv_body_h3", Desc: "Advanced: Body Types over H3/QUIC", Run: TestBodyH3},

		// Heavy Streaming Tests
		{Name: "l7_test_stream_h2_to_h3_1gb", Desc: "Streaming: 1GB H2 Client -> H3 Upstream", Run: TestStreamH2toH3},
		{Name: "l7_test_stream_h3_to_h2_1gb", Desc: "Streaming: 1GB H3 Client -> H2 Upstream", Run: TestStreamH3toH2},

		// CGI Tests
		{Name: "l7_test_cgi_binary_c", Desc: "CGI: Binary Execution (C Compiled)", Run: TestCgiBasic},
		{Name: "l7_test_cgi_script_lua", Desc: "CGI: Script Execution (Lua Interpreter)", Run: TestCgiLua},
		{Name: "l7_test_external_api_registration", Desc: "External Plugin: API Registration & Hot Reload", Run: TestExternalApiRegistration},
		{Name: "l7_test_external_persistence", Desc: "External Plugin: Persistence across restarts", Run: TestExternalPersistence},

		// WebSocket Tunneling Tests
		{Name: "l7_test_ws_deny_default", Desc: "WebSocket: Should reject if disabled", Run: TestWSDeny},
		{Name: "l7_test_ws_allow_echo", Desc: "WebSocket: Basic Echo Tunneling", Run: TestWSAllow},
		{Name: "l7_test_ws_stream_1gb", Desc: "WebSocket: 1GB Bidirectional Streaming", Run: TestWSStreamLarge},
		{Name: "l7_test_upstream_pool", Desc: "HTTP Upstream: Connection Pooling", Run: TestUpstreamConnectionPooling},
		{Name: "l7_test_static_basic", Desc: "Static Server: Basic index.html serving", Run: TestStaticServeBasic},
		{Name: "l7_test_static_range", Desc: "Static Server: HTTP Range (Partial Content)", Run: TestStaticRange},
		{Name: "l7_test_static_spa", Desc: "Static Server: SPA Fallback", Run: TestStaticSPA},
		{Name: "l7_test_static_browse", Desc: "Static Server: Directory Browsing", Run: TestStaticBrowse},
		{Name: "l7_test_static_traversal", Desc: "Static Server: Path Traversal Prevention", Run: TestStaticTraversal},
		{Name: "l7_test_static_precompressed", Desc: "Static Server: Precompressed .gz serving", Run: TestStaticPrecompressed},
		{Name: "l7_test_template_headers", Desc: "Template: Header Hijacking", Run: TestTemplateHeaderHijacking},
		{Name: "l7_test_template_body", Desc: "Template: Body Hijacking", Run: TestTemplateBodyHijacking},
		{Name: "l7_test_template_nested", Desc: "Template: Nested variable resolution", Run: TestTemplateNested},
		{Name: "l7_test_template_limit", Desc: "Template: Recursion depth limit", Run: TestTemplateRecursionLimit},
		{Name: "l7_test_template_json", Desc: "Template: Resolution inside JSON Input", Run: TestTemplateJsonResolution},
	}
}
