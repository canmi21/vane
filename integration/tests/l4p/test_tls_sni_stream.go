/* integration/tests/l4p/test_tls_sni_stream.go */
package l4p

import (
	"bufio"
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTlsSniStream(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Upstream (TLS Echo Server)
	// Server accepts generic TLS connections.
	srv, err := mock.NewTlsEchoServer([]string{"http/1.1"})
	if err != nil {
		return err
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	targetSni := "stream.vane.local"

	// 2. Configure Vane
	// L4: Upgrade to TLS (Peeking at ClientHello)
	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// L4+: If SNI matches, Proxy blindly (Passthrough). Else Abort.
	// Vane does NOT terminate TLS here; it forwards bytes.
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{tls.sni}}",
			targetSni,
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}
	l4pBytes, _ := json.Marshal(l4pFlow)
	s.WriteConfig("resolver/tls.json", l4pBytes)

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// 4. Positive Test: Long-lived Connection with Correct SNI
	tlsConf := &tls.Config{
		ServerName:         targetSni,
		InsecureSkipVerify: true, // We don't have the CA for the mock, just testing routing
	}

	conn, err := tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), tlsConf)
	if err != nil {
		return term.FormatFailure("TLS Handshake Failed", term.NewNode(err.Error()))
	}
	defer conn.Close()

	// Verify Stream Stability (Ping-Pong multiple times)
	reader := bufio.NewReader(conn)
	for i := 0; i < 5; i++ {
		payload := fmt.Sprintf("ping-%d\n", i)
		if _, err := conn.Write([]byte(payload)); err != nil {
			return term.FormatFailure(fmt.Sprintf("Write failed at iter %d", i), term.NewNode(err.Error()))
		}

		line, err := reader.ReadString('\n')
		if err != nil {
			return term.FormatFailure(fmt.Sprintf("Read failed at iter %d", i), term.NewNode(err.Error()))
		}

		if line != payload {
			return term.FormatFailure("Data corruption", term.NewNode(fmt.Sprintf("Sent: %q, Got: %q", payload, line)))
		}
	}

	// 5. Negative Test: Wrong SNI
	wrongConf := &tls.Config{
		ServerName:         "wrong.target",
		InsecureSkipVerify: true,
	}
	// Dial should fail (Abort) or Handshake should fail (Connection closed by Vane)
	badConn, err := tls.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), wrongConf)
	if err == nil {
		badConn.Close()
		// If Dial succeeds, try to write. Vane might accept TCP but close immediately on logic.
		// However, standard tls.Dial performs handshake. If Vane aborts, handshake fails.
		return term.FormatFailure("Expected TLS Handshake failure for wrong SNI", nil)
	}

	return nil
}
