/* integration/tests/l4p/suite.go */
package l4p

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

type TestEntry struct {
	Name string
	Desc string
	Run  func(context.Context, *env.Sandbox) error
}

// L4+ involves protocol-aware logic like SNI/ALPN routing.
func GetTests() []TestEntry {
	return []TestEntry{
		{
			Name: "l4p_test_tls_alpn_routing",
			Desc: "Route TCP traffic based on TLS ALPN negotiation",
			Run:  TestTlsAlpnProxy,
		},
		{
			Name: "l4p_test_quic_sni_routing",
			Desc: "Route UDP traffic based on QUIC ClientHello SNI (Simple)",
			Run:  TestQuicSniProxy,
		},
		{
			Name: "l4p_test_tls_sni_stream",
			Desc: "Long-lived TCP passthrough proxy based on SNI",
			Run:  TestTlsSniStream,
		},
		{
			Name: "l4p_test_quic_sni_stream",
			Desc: "Long-lived UDP/QUIC passthrough proxy based on SNI",
			Run:  TestQuicSniStream,
		},
	}
}
