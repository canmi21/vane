/* test/integration/tests/l4/tcp_ip_filtering_test.go */

package l4

import (
	"bufio"
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

func TestTcpIpFiltering(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Backend
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	// 2. Determine Vane Port
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
	}
	vanePort := ports[0]

	// 3. Helper to generate config with a specific Allowed IP
	genConfig := func(allowedIp string) []byte {
		// Flow: Match(IP) -> True: Proxy, False: Abort

		// Step 3: Abort (False branch) - Implicitly handled by flow ending,
		// but let's be explicit if possible. Actually, if output branch is missing, flow stops.
		// "Abort" plugin exists? internal.transport.abort?
		// Let's rely on default behavior: if "false" branch is taken and no step is there, flow ends.

		// Step 2: Proxy (True branch)
		proxyStep := advanced.ProcessingStep{
			"internal.transport.proxy": advanced.PluginInstance{
				Input: map[string]interface{}{
					"target.ip":   "127.0.0.1",
					"target.port": upstream.Port,
				},
			},
		}

		// Step 1: Matcher
		flowConf := advanced.L4FlowConfig{
			Connection: advanced.ProcessingStep{
				"internal.common.match": advanced.PluginInstance{
					Input: map[string]interface{}{
						"left":     "{{conn.ip}}",
						"right":    allowedIp,
						"operator": "contains", // Changed from eq to contains to handle ::ffff:127.0.0.1
					},
					Output: map[string]advanced.ProcessingStep{
						"true": proxyStep,
						// "false": empty -> connection closes
					},
				},
			},
		}

		bytes, _ := json.Marshal(flowConf)
		return bytes
	}

	// 4. Scenario A: Deny (Config allow=1.2.3.4, We are 127.0.0.1)
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), genConfig("1.2.3.4")); err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// Test Deny
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
	if err == nil {
		// Connection accepted (TCP handshake works), but it should be closed immediately or yield no data.
		conn.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
		buf := make([]byte, 10)
		n, err := conn.Read(buf)
		conn.Close()
		if err == nil && n > 0 {
			t.Fatal(term.FormatFailure("Traffic allowed when IP should be denied", nil))
		}
	}
	// Dial failing is also acceptable (e.g. if Vane closes aggressively)

	// 5. Scenario B: Allow (Config allow=127.0.0.1)
	// Update Config
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), genConfig("127.0.0.1")); err != nil {
		t.Fatal(err)
	}

	// Wait for reload signal explicitly
	if err := proc.WaitForLog("Config change signal", 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Vane did not reload config", term.NewNode(err.Error())))
	}
	// Allow a little extra time for the actual diff/apply
	time.Sleep(1 * time.Second)

	// Test Allow
	// Since Vane restarts the listener on config change, we might hit a brief "connection refused".
	var conn2 net.Conn
	retryDeadline := time.Now().Add(5 * time.Second)
	for {
		conn2, err = net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
		if err == nil {
			break
		}
		if time.Now().After(retryDeadline) {
			t.Fatal(term.FormatFailure("Failed to connect for Allow test after reload", term.NewNode(err.Error()+"\n\n--- Vane Logs ---\n"+proc.DumpLogs())))
		}
		time.Sleep(200 * time.Millisecond)
	}
	defer conn2.Close()

	fmt.Fprintf(conn2, "ping\n")
	resp, err := bufio.NewReader(conn2).ReadString('\n')
	if err != nil || resp != "ping\n" {
		t.Fatal(term.FormatFailure("Traffic denied/broken when IP should be allowed", nil))
	}
}
