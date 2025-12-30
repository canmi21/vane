/* integration/tests/l4/test_tcp_flow.go */
package l4

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTcpFlowProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Upstream Mock
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer upstream.Close()

	// 2. Setup Vane Config (JSON Flow)
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	vanePort := ports[0]

	// Create a simple flow: Connection -> Transparent Proxy -> Upstream
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewTransparentProxy("127.0.0.1", upstream.Port),
	}

	jsonBytes, err := json.Marshal(flowConf)
	if err != nil {
		return err
	}

	// Note the extension .json
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), jsonBytes); err != nil {
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

	payload := "FlowTCP\n"
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
