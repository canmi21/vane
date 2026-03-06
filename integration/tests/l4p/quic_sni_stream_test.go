/* integration/tests/l4p/quic_sni_stream_test.go */
package l4p

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	quic "github.com/quic-go/quic-go"
)

func TestQuicSniStream(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Upstream (QUIC Echo Server)
	srv, err := mock.NewQuicEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	targetSni := "stream.quic.local"

	// 2. Configure Vane
	// L4: Upgrade UDP to QUIC (Parsing Initial Packets)
	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("quic"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), l4Bytes)

	// L4+: SNI Routing -> Transparent Proxy
	// Vane acts as a NAT/Router for the QUIC stream after routing decision.
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{quic.sni}}",
			targetSni,
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}
	l4pBytes, _ := json.Marshal(l4pFlow)
	sb.WriteConfig("resolver/quic.json", l4pBytes)

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error())))
	}

	// 4. Positive Test: Long-lived QUIC Stream
	tlsConf := &tls.Config{
		ServerName:         targetSni,
		InsecureSkipVerify: true,
		NextProtos:         []string{"quic-echo"},
	}

	// Use a dedicated context for dialing to distinguish timeout sources
	dialCtx, dialCancel := context.WithTimeout(ctx, 5*time.Second)
	defer dialCancel()

	conn, err := quic.DialAddr(dialCtx, fmt.Sprintf("127.0.0.1:%d", vanePort), tlsConf, nil)
	if err != nil {
		t.Fatal(term.FormatFailure("QUIC Dial Failed", term.NewNode(err.Error())))
	}

	stream, err := conn.OpenStreamSync(ctx)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to open stream", term.NewNode(err.Error())))
	}
	defer stream.Close()

	// Verify multiple exchanges to ensure CID/Migration mapping works for ongoing flow
	buf := make([]byte, 1024)
	for i := 0; i < 5; i++ {
		msg := fmt.Sprintf("quic-ping-%d", i)
		if _, err := stream.Write([]byte(msg)); err != nil {
			t.Fatal(term.FormatFailure("Write failed", term.NewNode(err.Error())))
		}

		n, err := stream.Read(buf)
		if err != nil {
			t.Fatal(term.FormatFailure("Read failed", term.NewNode(err.Error())))
		}

		response := string(buf[:n])
		if response != msg {
			t.Fatal(term.FormatFailure("Payload mismatch", term.NewNode(fmt.Sprintf("Sent: %s, Got: %s", msg, response))))
		}
	}

	// Close session gracefully
	conn.CloseWithError(0, "bye")

	// 5. Negative Test: Wrong SNI
	// Should time out or receive immediate connection close frame depending on Vane's abort impl
	wrongConf := &tls.Config{
		ServerName:         "wrong.quic",
		InsecureSkipVerify: true,
		NextProtos:         []string{"quic-echo"},
	}

	failCtx, failCancel := context.WithTimeout(ctx, 1*time.Second)
	defer failCancel()

	_, err = quic.DialAddr(failCtx, fmt.Sprintf("127.0.0.1:%d", vanePort), wrongConf, nil)
	if err == nil {
		t.Fatal(term.FormatFailure("Expected QUIC failure for wrong SNI", nil))
	}
}
