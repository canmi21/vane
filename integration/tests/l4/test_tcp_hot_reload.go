/* integration/tests/l4/test_tcp_hot_reload.go */
package l4

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

func TestTcpHotReload(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start one robust upstream server
	srv1, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer srv1.Close()

	// Pre-check upstream health
	if !verifyDirectEcho(srv1.Port, "pre-check") {
		return term.FormatFailure("Mock Server is unhealthy directly!", nil)
	}

	// 2. Allocate TWO ports for Vane (Port A and Port B)
	ports, err := env.GetFreePorts(2)
	if err != nil {
		return err
	}
	portA := ports[0]
	portB := ports[1]

	// Helper to generate config bytes
	genConf := func(targetPort int) []byte {
		conf := basic.LegacyTcpConfig{
			Protocols: []basic.TcpProtocolRule{
				{
					Name:     "hotswaptest",
					Priority: 1,
					Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
					Destination: basic.TcpDestination{
						Type: "forward",
						Forward: &basic.Forward{
							Strategy: basic.StrategyRandom,
							Targets:  []basic.Target{{Ip: "127.0.0.1", Port: targetPort}},
						},
					},
				},
			},
		}
		bytes, _ := yaml.Marshal(conf)
		return bytes
	}

	// 3. Initial State: Only Port A Configured
	configPathA := fmt.Sprintf("listener/[%d]/tcp.yaml", portA)
	if err := s.WriteConfig(configPathA, genConf(srv1.Port)); err != nil {
		return err
	}

	// 4. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for Port A UP
	if err := proc.WaitForTcpPort(portA, 5*time.Second); err != nil {
		return term.FormatFailure("Port A failed to start", term.NewNode(err.Error()))
	}

	// Verify Port A
	if !verifyTcpEchoWithRetry(portA, "ping-A", 5) {
		return term.FormatFailure("Connection to Port A failed", nil)
	}

	// 5. THE SWITCH: Delete Config A, Create Config B
	// Vane has a 2-second debounce, so we can do file ops sequentially.

	// Delete A
	fullPathA := filepath.Join(s.ConfigDir, fmt.Sprintf("listener/[%d]/tcp.yaml", portA))
	if err := os.Remove(fullPathA); err != nil {
		return term.FormatFailure("Failed to remove config A", term.NewNode(err.Error()))
	}

	// Create B
	configPathB := fmt.Sprintf("listener/[%d]/tcp.yaml", portB)
	if err := s.WriteConfig(configPathB, genConf(srv1.Port)); err != nil {
		return err
	}

	// 6. Wait for Vane to react
	// Expecting "Config change signal" -> "PORT A DOWN" -> "PORT B UP"

	if err := proc.WaitForLog("Config change signal", 4*time.Second); err != nil {
		return term.FormatFailure("Vane did not detect config change", term.NewNode(err.Error()))
	}

	// Wait for Port A Down
	if err := proc.WaitForLog(fmt.Sprintf("PORT %d TCP DOWN", portA), 2*time.Second); err != nil {
		return term.FormatFailure("Vane did not stop Port A", term.NewNode(err.Error()))
	}

	// Wait for Port B Up
	if err := proc.WaitForTcpPort(portB, 5*time.Second); err != nil {
		return term.FormatFailure("Vane did not start Port B", term.NewNode(err.Error()))
	}

	// 7. Final Verification

	// Port A should be closed (Connection Refused)
	if verifyTcpEchoWithRetry(portA, "should-fail", 1) {
		return term.FormatFailure("Port A is still accepting connections!", nil)
	}

	// Port B should work
	if !verifyTcpEchoWithRetry(portB, "ping-B", 10) {
		return term.FormatFailure("Connection to Port B failed", nil)
	}

	return nil
}

// verifyDirectEcho talks directly to the backend
func verifyDirectEcho(port int, msg string) bool {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 200*time.Millisecond)
	if err != nil {
		return false
	}
	defer conn.Close()
	fmt.Fprintf(conn, "%s\n", msg)
	resp, err := bufio.NewReader(conn).ReadString('\n')
	return err == nil && resp == msg+"\n"
}

// verifyTcpEchoWithRetry talks to Vane with retries
func verifyTcpEchoWithRetry(port int, msg string, maxRetries int) bool {
	target := fmt.Sprintf("127.0.0.1:%d", port)

	for i := 0; i < maxRetries; i++ {
		conn, err := net.DialTimeout("tcp", target, 200*time.Millisecond)
		if err != nil {
			// If we expect failure (maxRetries=1), this is good.
			// If we expect success, we wait.
			time.Sleep(100 * time.Millisecond)
			continue
		}

		conn.SetDeadline(time.Now().Add(500 * time.Millisecond))
		fmt.Fprintf(conn, "%s\n", msg)

		resp, err := bufio.NewReader(conn).ReadString('\n')
		conn.Close()

		if err == nil && resp == msg+"\n" {
			return true
		}
		time.Sleep(100 * time.Millisecond)
	}
	return false
}
