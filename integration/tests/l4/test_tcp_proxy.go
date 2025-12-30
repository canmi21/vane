/* integration/tests/l4/test_tcp_proxy.go */
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

func TestTcpProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Upstream Mock
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer upstream.Close()

	// 2. Setup Vane Config
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	vanePort := ports[0]

	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				// FIXED: Name must be strictly [a-z0-9]+
				Name:     "echoservice",
				Priority: 10,
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
		return err
	}
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		return err
	}

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// 4. Test Traffic
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		return term.FormatFailure("Failed to dial Vane", term.NewNode(err.Error()))
	}
	defer conn.Close()

	payload := "Hello Vane\n"
	fmt.Fprintf(conn, "%s", payload)

	response, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		return term.FormatFailure("Failed to read from Vane", term.NewNode(err.Error()))
	}

	if response != payload {
		root := term.NewNode("Data Mismatch")
		root.Add(fmt.Sprintf("Sent: %q", payload))
		root.Add(fmt.Sprintf("Recv: %q", response))
		return term.FormatFailure("Echo mismatch", root)
	}

	return nil
}
