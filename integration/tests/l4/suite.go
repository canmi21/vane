/* integration/tests/l4/suite.go */
package l4

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

type TestFunc func(ctx context.Context, s *env.Sandbox) error

type TestCase struct {
	Name string
	Desc string
	Run  TestFunc
}

func GetTests() []TestCase {
	return []TestCase{
		{Name: "l4_test_tcp_binding", Desc: "Verifies TCP binding", Run: TestTcpBinding},
		{Name: "l4_test_tcp_proxy", Desc: "Verifies TCP proxy", Run: TestTcpProxy},
		{Name: "l4_test_udp_proxy", Desc: "Verifies UDP proxy", Run: TestUdpProxy},
		{Name: "l4_test_tcp_hot_reload", Desc: "Verifies TCP reload", Run: TestTcpHotReload},
		{Name: "l4_test_udp_hot_reload", Desc: "Verifies UDP reload", Run: TestUdpHotReload},
		{Name: "l4_test_tcp_flow", Desc: "Verifies TCP Flow JSON", Run: TestTcpFlowProxy},
		{Name: "l4_test_udp_flow", Desc: "Verifies UDP Flow JSON", Run: TestUdpFlowProxy},
		{Name: "l4_test_tcp_flow_detect_http", Desc: "Verifies L4 HTTP Detection", Run: TestTcpFlowDetectHttp},
		{Name: "l4_test_tcp_flow_detect_tls", Desc: "Verifies L4 TLS Detection", Run: TestTcpFlowDetectTls},
		{Name: "l4_test_udp_flow_detect_dns", Desc: "Verifies L4 DNS Detection", Run: TestUdpFlowDetectDns},
		{Name: "l4_test_udp_flow_detect_quic", Desc: "Verifies L4 QUIC Detection", Run: TestUdpFlowDetectQuic},
		{Name: "l4_test_tcp_flow_ratelimit", Desc: "Verifies L4 Rate Limiting", Run: TestTcpFlowRateLimit},
		{Name: "l4_test_backend_recovery", Desc: "Verifies Backend Auto Recovery", Run: TestBackendRecovery},
		{Name: "l4_test_tcp_fallback", Desc: "Verifies TCP fallback targets", Run: TestTcpFallback},
		{Name: "l4_test_tcp_protocol_filtering", Desc: "Verifies protocol-based filtering", Run: TestTcpProtocolFiltering},
		{Name: "l4_test_tcp_ip_filtering", Desc: "Verifies IP-based filtering (Flow Matcher)", Run: TestTcpIpFiltering},
	}
}
