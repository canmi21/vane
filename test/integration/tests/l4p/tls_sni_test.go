/* test/integration/tests/l4p/tls_sni_test.go */

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

func TestTlsSniProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Use a TLS Backend so the handshake succeeds
	srv, err := mock.NewTlsEchoServer([]string{"http/1.1"}) // Generic ALPN
	if err != nil {
		t.Fatal(err)
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// 1. L4: Upgrade to TLS
	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// 2. L4+: IF {{tls.sni}} == "secure.internal" THEN Proxy ELSE Abort
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{tls.sni}}",
			"secure.internal",
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

	// 3. Positive Test
	conn, err := tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		ServerName:         "secure.internal",
		InsecureSkipVerify: true,
	})
	if err != nil {
		t.Fatal(term.FormatFailure("TLS Handshake Failed (Correct SNI)", term.NewNode(err.Error())))
	}

	fmt.Fprintf(conn, "hello\n")
	resp, err := bufio.NewReader(conn).ReadString('\n')
	conn.Close()

	// Expect echo
	if err != nil || resp != "hello\n" {
		t.Fatal(term.FormatFailure("Traffic check failed", term.NewNode(fmt.Sprintf("Got: %s, Err: %v", resp, err))))
	}

	// 4. Negative Test
	_, err = tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		ServerName:         "wrong.com",
		InsecureSkipVerify: true,
	})
	if err == nil {
		t.Fatal(term.FormatFailure("Expected connection failure for wrong SNI, but got success", nil))
	}
}
