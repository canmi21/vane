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

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// Verify connection
	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := net.DialTimeout("tcp", target, 1*time.Second)
	if err != nil {
		root := term.NewNode("")
		root.Add("Details: Failed to connect to TCP port")
		root.Add(fmt.Sprintf("Error: %v", err))
		if !debug {
			root.Add("Logs").Add(proc.DumpLogs())
		}
		return term.FormatFailure("Binding check failed", root)
	}
	conn.Close()

	return nil
}
