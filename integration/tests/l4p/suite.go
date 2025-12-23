/* integration/tests/l4p/suite.go */
package l4p

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
		{Name: "l4p_test_tls_sni_proxy", Desc: "L4+ TLS SNI Routing", Run: TestTlsSniProxy},
		{Name: "l4p_test_tls_alpn_proxy", Desc: "L4+ TLS ALPN Routing", Run: TestTlsAlpnProxy},
		{Name: "l4p_test_http_host_proxy", Desc: "L4+ HTTP Host Routing", Run: TestHttpHostProxy},
		{Name: "l4p_test_quic_sni_proxy", Desc: "L4+ QUIC SNI Routing", Run: TestQuicSniProxy},
	}
}
