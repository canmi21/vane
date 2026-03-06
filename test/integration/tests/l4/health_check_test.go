/* test/integration/tests/l4/health_check_test.go */

package l4

import (
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

// TestTcpHealthCheck verifies that Vane detects a down backend and recovers when it returns.
func TestTcpHealthCheck(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Backend
	backend, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	// We don't defer Close() immediately because we need to manually close it during test.
	backendPort := backend.Port

	// 2. Configure Vane
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	// Set faster health check interval
	sb.Env["HEALTH_TCP_INTERVAL_SECS"] = "1"

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "tcp_health",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Ip: "127.0.0.1", Port: backendPort},
						},
					},
				},
			},
		},
	}

	confBytes, _ := yaml.Marshal(config)
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), confBytes); err != nil {
		backend.Close()
		t.Fatal(err)
	}

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		backend.Close()
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		backend.Close()
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// Helper to check connectivity
	checkConn := func() error {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
		if err != nil {
			return err
		}
		conn.Close()
		return nil
	}

	// 4. Verify Initial Health
	time.Sleep(2 * time.Second) // Wait for initial check
	if err := checkConn(); err != nil {
		backend.Close()
		t.Fatal(term.FormatFailure("Initial connection failed", term.NewNode(err.Error())))
	}

	// 5. Kill Backend
	backend.Close()
	time.Sleep(2 * time.Second) // Wait for health check to detect failure

	// 6. Verify Traffic Fails (Vane should close connection or refuse, or timeout if it holds it?)
	// If no targets available, Vane logs warning and closes connection.
	// We expect Dial to succeed (Vane is listening), but Read/Write/Connect to backend to fail.
	// Wait, if Vane accepts connection but drops it, Dial succeeds.
	// We need to check if data flows.
	// Actually, if Vane finds no targets, it might close immediately.

	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
	if err == nil {
		// Connection established with Vane. Check if it closes.
		one := make([]byte, 1)
		conn.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
		_, err := conn.Read(one)
		conn.Close()
		// If read succeeds (got data), that's bad (ghost?). If read fails with EOF, Vane closed it.
		// If read times out, Vane kept it open?
		if err == nil {
			t.Fatal(term.FormatFailure("Got data when backend is down", nil))
		}
		// Any error is "good" here (EOF or Timeout), implies backend didn't echo.
	}

	// 7. Restart Backend
	// Use helper from test_backend_recovery logic or similar
	l, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", backendPort))
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to restart backend", term.NewNode(err.Error())))
	}
	newBackend := mock.NewTcpEchoServerFromListener(l)
	defer newBackend.Close()

	// 8. Verify Recovery
	time.Sleep(3 * time.Second) // Wait for health check (interval 1s + buffer)

	// Retry loop for recovery verification
	recovered := false
	for i := 0; i < 5; i++ {
		if err := checkConn(); err == nil {
			recovered = true
			break
		}
		time.Sleep(500 * time.Millisecond)
	}

	if !recovered {
		t.Fatal(term.FormatFailure("Failed to recover after backend restart", term.NewNode(proc.DumpLogs())))
	}
}
