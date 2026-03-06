/* test/integration/tests/common/no_console_test.go */

package common

import (
	"bufio"
	"fmt"
	"net"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

// TestNoConsole verifies that Vane starts correctly without ACCESS_TOKEN:
// - Management console port should NOT be listening
// - Business ports should still work normally
// - Logs should show "Access token not set, management API disabled" message
func TestNoConsole(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Ensure clean slate
	delete(sb.Env, "ACCESS_TOKEN")

	// 1. Setup upstream mock echo server
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	// 2. Allocate a business port for testing
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
	}
	vanePort := ports[0]

	// 3. Create TCP config using the standard format
	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "noconsoletest",
				Priority: 1,
				Detect: basic.Detect{
					Method:  basic.DetectFallback,
					Pattern: "any",
				},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Ip: "127.0.0.1", Port: upstream.Port},
						},
					},
				},
			},
		},
	}

	bytes, err := yaml.Marshal(tcpConf)
	if err != nil {
		t.Fatal(err)
	}

	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		t.Fatal(err)
	}

	// 4. Start Vane WITHOUT ACCESS_TOKEN
	// The startup process already waited for "Access token not set, management API disabled" log and business port initialization
	proc, err := sb.StartVaneWithoutToken(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// 5. Verify console port is NOT listening
	consoleTarget := fmt.Sprintf("127.0.0.1:%d", sb.ConsolePort)
	consoleConn, err := net.DialTimeout("tcp", consoleTarget, 100*time.Millisecond)
	if err == nil {
		consoleConn.Close()
		t.Fatal(term.FormatFailure("Console port should NOT be listening", term.NewNode(fmt.Sprintf("Port %d is accepting connections", sb.ConsolePort))))
	}

	// 6. Wait for business port to become available
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Business port failed to start", term.NewNode(err.Error())))
	}

	// 7. Verify business port works normally (echo test)
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to connect to business port", term.NewNode(err.Error())))
	}
	defer conn.Close()

	testPayload := "no-console-test\n"
	fmt.Fprintf(conn, "%s", testPayload)

	response, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to read from business port", term.NewNode(err.Error())))
	}

	if response != testPayload {
		root := term.NewNode("Echo Mismatch")
		root.Add(fmt.Sprintf("Sent: %q", testPayload))
		root.Add(fmt.Sprintf("Recv: %q", response))
		t.Fatal(term.FormatFailure("Business port echo test failed", root))
	}
}
