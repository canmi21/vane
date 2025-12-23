/* integration/tests/l4p/test_tls_sni.go */
package l4p

import (
	"bufio"
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTlsSniProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Use a TLS Backend so the handshake succeeds
	srv, err := mock.NewTlsEchoServer([]string{"http/1.1"}) // Generic ALPN
	if err != nil {
		return err
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// 1. L4: Upgrade to TLS
	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

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
	s.WriteConfig("resolver/tls.json", l4pBytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 3. Positive Test
	conn, err := tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		ServerName:         "secure.internal",
		InsecureSkipVerify: true,
	})
	if err != nil {
		return term.FormatFailure("TLS Handshake Failed (Correct SNI)", term.NewNode(err.Error()))
	}

	fmt.Fprintf(conn, "hello\n")
	resp, err := bufio.NewReader(conn).ReadString('\n')
	conn.Close()

	// Expect echo
	if err != nil || resp != "hello\n" {
		return term.FormatFailure("Traffic check failed", term.NewNode(fmt.Sprintf("Got: %s, Err: %v", resp, err)))
	}

	// 4. Negative Test
	_, err = tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), &tls.Config{
		ServerName:         "wrong.com",
		InsecureSkipVerify: true,
	})
	if err == nil {
		return term.FormatFailure("Expected connection failure for wrong SNI, but got success", nil)
	}

	return nil
}
