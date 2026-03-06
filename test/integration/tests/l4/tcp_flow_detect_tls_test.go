/* test/integration/tests/l4/tcp_flow_detect_tls_test.go */

package l4

import (
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTcpFlowDetectTls(t *testing.T) {
	sb, ctx := env.SetupTest(t)
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
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), jsonBytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 4. Positive Test: Send TLS ClientHello Header (0x16 0x03 ...)
	// FIXED: Append 0x0A (\n) because the Mock TCP Server uses bufio.Scanner
	// which waits for a newline to process and echo the data.
	tlsHeader := string([]byte{0x16, 0x03, 0x01, 0x00, 0xA0, 0x0A})

	if err := verifyTcpConnection(vanePort, tlsHeader, true); err != nil {
		t.Fatal(term.FormatFailure("Positive Check Failed (TLS)", term.NewNode(err.Error())))
	}

	// 5. Negative Test: Send Garbage
	if err := verifyTcpConnection(vanePort, "NOT_TLS_JUNK\n", false); err != nil {
		t.Fatal(term.FormatFailure("Negative Check Failed (Garbage)", term.NewNode(err.Error())))
	}
}
