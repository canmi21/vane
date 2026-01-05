/* integration/tests/l4/test_tcp_fallback.go */
package l4

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

func TestTcpFallback(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Servers
	primary, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer primary.Close()

	secondary, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer secondary.Close()

	// 2. Setup Vane Config (Legacy)
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	vanePort := ports[0]

	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "fallbacktest",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Ip: "127.0.0.1", Port: primary.Port},
						},
						Fallbacks: []basic.Target{
							{Ip: "127.0.0.1", Port: secondary.Port},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(tcpConf)
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		return err
	}

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// Helper to verify which server responded
	// We'll use a unique message and assume echo works.
	// Since we can't distinguish between servers easily by content,
	// we will rely on the fact that if we stop primary, ONLY secondary is left.
	verifyEcho := func(port int, msg string) error {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 500*time.Millisecond)
		if err != nil {
			return err
		}
		defer conn.Close()
		fmt.Fprintf(conn, "%s\n", msg)
		resp, err := bufio.NewReader(conn).ReadString('\n')
		if err != nil {
			return err
		}
		if resp != msg+"\n" {
			return fmt.Errorf("echo mismatch: got %q, want %q", resp, msg)
		}
		return nil
	}

	// 4. Verify Primary is working
	if err := verifyEcho(vanePort, "hello-primary"); err != nil {
		return term.FormatFailure("Initial connection failed", term.NewNode(err.Error()))
	}

	// 5. KILL Primary
	primary.Close()

	// 6. Verify Fallback to Secondary
	// Vane should detect the failure on dial and switch to fallback immediately
	// or after a few retries if the balancer handles it.
	var lastErr error
	for i := 0; i < 5; i++ {
		lastErr = verifyEcho(vanePort, fmt.Sprintf("hello-fallback-%d", i))
		if lastErr == nil {
			break
		}
		time.Sleep(200 * time.Millisecond)
	}

	if lastErr != nil {
		return term.FormatFailure("Fallback failed: Secondary not reachable through Vane", term.NewNode(lastErr.Error()))
	}

	return nil
}
