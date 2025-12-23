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
		// Basic
		{Name: "l7_test_https_proxy_basic", Desc: "Basic HTTPS Proxy (H1->H1)", Run: TestHttpsProxy},

		// Matrix: Client H1
		{Name: "l7_test_h1_to_h1", Desc: "Matrix: H1 Client -> H1 Upstream", Run: TestH1toH1},
		{Name: "l7_test_h1_to_h2", Desc: "Matrix: H1 Client -> H2 Upstream", Run: TestH1toH2},
		{Name: "l7_test_h1_to_h3", Desc: "Matrix: H1 Client -> H3 Upstream", Run: TestH1toH3},

		// Matrix: Client H2
		{Name: "l7_test_h2_to_h1", Desc: "Matrix: H2 Client -> H1 Upstream", Run: TestH2toH1},
		{Name: "l7_test_h2_to_h2", Desc: "Matrix: H2 Client -> H2 Upstream", Run: TestH2toH2},
		{Name: "l7_test_h2_to_h3", Desc: "Matrix: H2 Client -> H3 Upstream", Run: TestH2toH3},

		// Matrix: Client H3
		{Name: "l7_test_h3_to_h1", Desc: "Matrix: H3 Client -> H1 Upstream", Run: TestH3toH1},
		{Name: "l7_test_h3_to_h2", Desc: "Matrix: H3 Client -> H2 Upstream", Run: TestH3toH2},
		{Name: "l7_test_h3_to_h3", Desc: "Matrix: H3 Client -> H3 Upstream", Run: TestH3toH3},
	}
}
