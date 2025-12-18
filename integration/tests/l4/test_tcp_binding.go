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

type TestFunc func(ctx context.Context, s *env.Sandbox) error

type TestCase struct {
	Name string
	Desc string
	Run  TestFunc
}

func GetTests() []TestCase {
	return []TestCase{
		{
			Name: "test_tcp_binding",
			Desc: "Verifies Vane can bind to a random TCP port from config",
			Run:  TestBasicBinding,
		},
	}
}

func TestBasicBinding(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Get 1 free port for TCP
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	tcpPort := ports[0]

	// --- Config Generation ---

	// Dummy target to satisfy validation (must have at least one target)
	dummyTarget := basic.Target{
		Ip:   "127.0.0.1",
		Port: 9999,
	}

	// TCP Config
	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "testtcp",
				Priority: 1,
				Detect: basic.Detect{
					Method:  basic.DetectFallback,
					Pattern: "any",
				},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy:  basic.StrategyRandom,
						Targets:   []basic.Target{dummyTarget},
						Fallbacks: []basic.Target{},
					},
				},
			},
		},
	}

	// Serialize to YAML
	tcpBytes, err := yaml.Marshal(tcpConf)
	if err != nil {
		return fmt.Errorf("failed to marshal tcp config: %w", err)
	}

	// Write Configs
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", tcpPort), tcpBytes); err != nil {
		return err
	}

	// Required files for startup
	if err := s.WriteConfig("nodes.yaml", []byte("nodes: []")); err != nil {
		return err
	}
	if err := s.WriteConfig("plugins.json", []byte("{}")); err != nil {
		return err
	}

	// --- Start Vane ---
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// --- Verify TCP Binding (With Retry) ---
	target := fmt.Sprintf("127.0.0.1:%d", tcpPort)
	var conn net.Conn
	var dialErr error

	for i := 0; i < 20; i++ { // Retry 20 times (2s total)
		conn, dialErr = net.DialTimeout("tcp", target, 200*time.Millisecond)
		if dialErr == nil {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	if dialErr != nil {
		root := term.NewNode("")
		scenario := root.Add("Test Scenario")
		scenario.Add(fmt.Sprintf("Action: Dial TCP %s", target))
		scenario.Add("Retries: 20x 100ms")

		result := root.Add("Result")
		result.Add(fmt.Sprintf("Error: %v", dialErr))
		result.Add("Status: Process running")

		if !debug {
			logs := root.Add("Logs (Snippet)")
			logs.Add(proc.DumpLogs())
		}

		return term.FormatFailure("Failed to connect to TCP port", root)
	}
	conn.Close()

	return nil
}
