/* integration/tests/mgmt/suite.go */
package mgmt

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
		{Name: "mgmt_console_http", Desc: "Management: Access Console via HTTP", Run: TestConsoleHttp},
		{Name: "mgmt_console_uds", Desc: "Management: Access Console via Unix Domain Socket", Run: TestConsoleUds},
		{Name: "mgmt_console_no_token", Desc: "Management: Console disabled without ACCESS_TOKEN", Run: TestConsoleNoToken},
	}
}
