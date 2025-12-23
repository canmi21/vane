/* integration/tests/l4/test_tcp_binding.go */
package l4

import (
	"context"
	"fmt"
	"net"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

func TestTcpBinding(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	vanePort := ports[0]

	// Config
	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				// FIXED: Name must be strictly [a-z0-9]+ (no underscores)
				Name:     "bindingtest",
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
							{Ip: "127.0.0.1", Port: 9999}, // Dummy
						},
					},
				},
			},
		},
	}

	bytes, err := yaml.Marshal(tcpConf)
	if err != nil {
		return err
	}

	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		return err
	}

	// Start
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Verify
	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	var dialErr error
	for i := 0; i < 20; i++ {
		conn, err := net.DialTimeout("tcp", target, 200*time.Millisecond)
		if err == nil {
			conn.Close()
			dialErr = nil
			break
		}
		dialErr = err
		time.Sleep(100 * time.Millisecond)
	}

	if dialErr != nil {
		root := term.NewNode("")
		root.Add("Details: Failed to connect to TCP port")
		root.Add(fmt.Sprintf("Error: %v", dialErr))
		if !debug {
			root.Add("Logs").Add(proc.DumpLogs())
		}
		return term.FormatFailure("Binding check failed", root)
	}

	return nil
}
