/* integration/tests/l4/test_udp_flow.go */
package l4

import (
	"bytes"
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

func TestUdpFlowProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	expectedResponse := []byte("FlowUDP-Response")

	// 1. Setup Upstream Mock
	upstream, err := mock.NewUdpFixedResponseServer(expectedResponse)
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

	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), jsonBytes); err != nil {
		return err
	}

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for UP
	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error()))
	}

	// 4. Send Packet
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", vanePort))
	if err != nil {
		return err
	}
	defer conn.Close()

	if _, err := conn.Write([]byte("ping")); err != nil {
		return err
	}

	conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	buf := make([]byte, 1024)
	n, err := conn.Read(buf)
	if err != nil {
		root := term.NewNode("UDP Read Error")
		root.Add(err.Error())
		if !debug {
			root.Add("Logs").Add(proc.DumpLogs())
		}
		return term.FormatFailure("No response from Vane UDP", root)
	}

	recv := buf[:n]
	if !bytes.Equal(recv, expectedResponse) {
		root := term.NewNode("Payload Mismatch")
		root.Add(fmt.Sprintf("Expected: %s", expectedResponse))
		root.Add(fmt.Sprintf("Actual:   %s", recv))
		return term.FormatFailure("Wrong UDP Response", root)
	}

	return nil
}
