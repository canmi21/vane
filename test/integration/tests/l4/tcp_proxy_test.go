/* test/integration/tests/l4/tcp_proxy_test.go */

package l4

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

func TestTcpProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Upstream Mock
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	// 2. Setup Vane Config
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
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
		t.Fatal(err)
	}
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		t.Fatal(err)
	}

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 4. Test Traffic
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to dial Vane", term.NewNode(err.Error())))
	}
	defer conn.Close()

	payload := "Hello Vane\n"
	fmt.Fprintf(conn, "%s", payload)

	response, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to read from Vane", term.NewNode(err.Error())))
	}

	if response != payload {
		root := term.NewNode("Data Mismatch")
		root.Add(fmt.Sprintf("Sent: %q", payload))
		root.Add(fmt.Sprintf("Recv: %q", response))
		t.Fatal(term.FormatFailure("Echo mismatch", root))
	}
}
