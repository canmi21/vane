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
		{
			Name: "l7_test_https_proxy_basic",
			Desc: "Full Stack: TCP->TLS->L7(HTTPX)->Upstream(H1)",
			Run:  TestHttpsProxy,
		},
	}
}
