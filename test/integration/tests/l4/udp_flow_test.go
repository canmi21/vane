/* test/integration/tests/l4/udp_flow_test.go */

package l4

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestUdpFlowProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	expectedResponse := []byte("FlowUDP-Response")

	// 1. Setup Upstream Mock
	upstream, err := mock.NewUdpFixedResponseServer(expectedResponse)
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	// 2. Setup Vane Config (JSON Flow)
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
	}
	vanePort := ports[0]

	// Create a simple flow: Connection -> Transparent Proxy -> Upstream
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewTransparentProxy("127.0.0.1", upstream.Port),
	}

	jsonBytes, err := json.Marshal(flowConf)
	if err != nil {
		t.Fatal(err)
	}

	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), jsonBytes); err != nil {
		t.Fatal(err)
	}

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for UP
	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error())))
	}

	// 4. Send Packet
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", vanePort))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	if _, err := conn.Write([]byte("ping")); err != nil {
		t.Fatal(err)
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
		t.Fatal(term.FormatFailure("No response from Vane UDP", root))
	}

	recv := buf[:n]
	if !bytes.Equal(recv, expectedResponse) {
		root := term.NewNode("Payload Mismatch")
		root.Add(fmt.Sprintf("Expected: %s", expectedResponse))
		root.Add(fmt.Sprintf("Actual:   %s", recv))
		t.Fatal(term.FormatFailure("Wrong UDP Response", root))
	}
}
