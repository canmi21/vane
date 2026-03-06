/* test/integration/tests/l4p/tls_alpn_test.go */

package l4p

import (
	"bufio"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTlsAlpnProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Use TLS Backend that supports "h2" so negotiation succeeds
	srv, err := mock.NewTlsEchoServer([]string{"h2"})
	if err != nil {
		t.Fatal(err)
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// L4+: IF {{tls.alpn}} == "h2" THEN Proxy ELSE Abort
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{tls.alpn}}",
			"h2",
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}
	l4pBytes, _ := json.Marshal(l4pFlow)
	sb.WriteConfig("resolver/tls.json", l4pBytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// Positive Test
	conn, err := tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		NextProtos:         []string{"h2"},
		InsecureSkipVerify: true,
	})
	if err != nil {
		t.Fatal(term.FormatFailure("ALPN Match Failed", term.NewNode(err.Error())))
	}

	// Verify ConnectionState negotiation
	state := conn.ConnectionState()
	if state.NegotiatedProtocol != "h2" {
		t.Fatal(term.FormatFailure("ALPN Negotiation failed", term.NewNode(fmt.Sprintf("Got: %s", state.NegotiatedProtocol))))
	}

	fmt.Fprintf(conn, "alpn-test\n")
	resp, _ := bufio.NewReader(conn).ReadString('\n')
	conn.Close()
	if resp != "alpn-test\n" {
		t.Fatal(term.FormatFailure("Data echo failed", nil))
	}

	// Negative Test
	_, err = tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		NextProtos:         []string{"http/1.1"},
		InsecureSkipVerify: true,
	})
	if err == nil {
		t.Fatal(term.FormatFailure("Expected failure for wrong ALPN", nil))
	}
}
