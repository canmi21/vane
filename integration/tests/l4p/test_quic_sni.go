/* integration/tests/l4p/test_quic_sni.go */
package l4p

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"github.com/quic-go/quic-go"
)

func TestQuicSniProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// QUIC Echo Server
	srv, err := mock.NewQuicEchoServer()
	if err != nil {
		return err
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// L4: Upgrade UDP -> "quic"
	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("quic"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), l4Bytes)

	// L4+ (resolver/quic.json): Match {{quic.sni}}
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{quic.sni}}",
			"quic.vane",
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}
	l4pBytes, _ := json.Marshal(l4pFlow)
	s.WriteConfig("resolver/quic.json", l4pBytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error()))
	}

	// --- Positive Test: Match SNI "quic.vane" ---
	tlsConf := &tls.Config{
		ServerName:         "quic.vane",
		InsecureSkipVerify: true,
		NextProtos:         []string{"h3", "quic-echo"},
	}

	// Use a separate timeout for Dial to distinguish Vane timeout from Test timeout
	dialCtx, dialCancel := context.WithTimeout(ctx, 3*time.Second)
	defer dialCancel()

	// NOTE: If this fails with Timeout, it means Vane dropped the packet
	// (likely because it failed to parse SNI and hit the default 'Abort' branch).
	conn, err := quic.DialAddr(dialCtx, fmt.Sprintf("127.0.0.1:%d", vanePort), tlsConf, nil)
	if err != nil {
		return term.FormatFailure("QUIC Handshake Failed (SNI Match)",
			term.NewNode(fmt.Sprintf("Error: %v (Vane likely failed to parse SNI)", err)))
	}

	stream, err := conn.OpenStreamSync(ctx)
	if err == nil {
		stream.Write([]byte("ping"))
		buf := make([]byte, 4)
		stream.Read(buf)
		stream.Close()
	}
	conn.CloseWithError(0, "ok")

	// --- Negative Test: Mismatch SNI ---
	tlsConfWrong := &tls.Config{
		ServerName:         "wrong.vane",
		InsecureSkipVerify: true,
		NextProtos:         []string{"h3"},
	}

	failCtx, failCancel := context.WithTimeout(ctx, 1*time.Second)
	defer failCancel()

	_, err = quic.DialAddr(failCtx, fmt.Sprintf("127.0.0.1:%d", vanePort), tlsConfWrong, nil)
	if err == nil {
		return term.FormatFailure("Expected QUIC failure for wrong SNI, but got success", nil)
	}

	return nil
}
