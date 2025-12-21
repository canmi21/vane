/* integration/tests/l4/suite.go */
package l4

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

// Shared definitions to avoid circular imports if needed in future
type TestFunc func(ctx context.Context, s *env.Sandbox) error

type TestCase struct {
	Name string
	Desc string
	Run  TestFunc
}

func GetTests() []TestCase {
	return []TestCase{
		{
			Name: "l4_test_tcp_binding",
			Desc: "Verifies Vane can bind to a random TCP port from config",
			Run:  TestTcpBinding,
		},
		{
			Name: "l4_test_tcp_proxy",
			Desc: "Verifies basic TCP forwarding to an upstream echo server",
			Run:  TestTcpProxy,
		},
		{
			Name: "l4_test_udp_proxy",
			Desc: "Verifies UDP forwarding using magic byte detection",
			Run:  TestUdpProxy,
		},
		{
			Name: "l4_test_tcp_hot_reload",
			Desc: "Verifies TCP listener reloads config without process restart",
			Run:  TestTcpHotReload,
		},
		{
			Name: "l4_test_udp_hot_reload",
			Desc: "Verifies UDP listener reloads config without process restart",
			Run:  TestUdpHotReload,
		},
		{
			Name: "l4_test_tcp_flow",
			Desc: "Verifies TCP Flow Engine with JSON config",
			Run:  TestTcpFlowProxy,
		},
		{
			Name: "l4_test_udp_flow",
			Desc: "Verifies UDP Flow Engine with JSON config",
			Run:  TestUdpFlowProxy,
		},
	}
}
