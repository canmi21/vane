/* integration/tests/l4/test_tcp_flow_detect_tls.go */
package l4

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTcpFlowDetectTls(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	srv, _ := mock.NewTcpEchoServer()
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// Detect(TLS) ? Proxy(Srv) : Abort()
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewProtocolDetect(
			"tls",
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}

	jsonBytes, _ := json.Marshal(flowConf)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), jsonBytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// 4. Positive Test: Send TLS ClientHello Header (0x16 0x03 ...)
	// FIXED: Append 0x0A (\n) because the Mock TCP Server uses bufio.Scanner
	// which waits for a newline to process and echo the data.
	tlsHeader := string([]byte{0x16, 0x03, 0x01, 0x00, 0xA0, 0x0A})

	if err := verifyTcpConnection(vanePort, tlsHeader, true); err != nil {
		return term.FormatFailure("Positive Check Failed (TLS)", term.NewNode(err.Error()))
	}

	// 5. Negative Test: Send Garbage
	if err := verifyTcpConnection(vanePort, "NOT_TLS_JUNK\n", false); err != nil {
		return term.FormatFailure("Negative Check Failed (Garbage)", term.NewNode(err.Error()))
	}

	return nil
}
