/* test/integration/tests/l7/external_generic_test.go */

package l7

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

const pluginGoSource = `
package main

import (
	"encoding/json"
	"io"
	"os"
)

func main() {
	inputBytes, _ := io.ReadAll(os.Stdin)
	var inputs map[string]interface{}
	if err := json.Unmarshal(inputBytes, &inputs); err != nil {
		os.Exit(1)
	}

	if inputs["test_val"] != "hello" {
		os.Stderr.WriteString("Invalid input value")
		os.Exit(1)
	}

	// Output MiddlewareOutput JSON
	out := map[string]interface{}{
		"branch": "true",
		"store": map[string]string{
			"ext_status": "verified",
		},
	}
	bytes, _ := json.Marshal(out)
	os.Stdout.Write(bytes)
}
`

// Test 1: Register via API and verify immediately
func TestExternalApiRegistration(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Backend & Compile
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	binPath, err := sb.CompileGoBin(pluginGoSource)
	if err != nil {
		t.Fatal(term.FormatFailure("Plugin Compilation Failed", term.NewNode(err.Error())))
	}

	// 2. Start Vane with Known Token
	token := "test-token-api-reg"
	sb.Env["ACCESS_TOKEN"] = token
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// 3. Register Plugin via API
	if err := registerPlugin(sb.ConsolePort, token, binPath); err != nil {
		t.Fatal(term.FormatFailure("API Registration Failed", term.NewNode(err.Error())))
	}

	// 4. Configure Listener (using hot-reload)
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	if err := writeListenerConfig(sb, vanePort, upstream.Port); err != nil {
		t.Fatal(err)
	}

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 5. Verify Traffic
	if err := verifyTraffic(vanePort); err != nil {
		t.Fatal(term.FormatFailure("Traffic Verification Failed", term.NewNode(err.Error())))
	}
}

// Test 2: Register via API, Restart, Verify Persistence
func TestExternalPersistence(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup & Compile
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	binPath, err := sb.CompileGoBin(pluginGoSource)
	if err != nil {
		t.Fatal(term.FormatFailure("Plugin Compilation Failed", term.NewNode(err.Error())))
	}

	// 2. Start Vane (Run 1)
	token := "test-token-persist"
	sb.Env["ACCESS_TOKEN"] = token
	proc1, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}

	// 3. Register Plugin via API
	if err := registerPlugin(sb.ConsolePort, token, binPath); err != nil {
		proc1.Stop()
		t.Fatal(term.FormatFailure("API Registration Failed (Run 1)", term.NewNode(err.Error())))
	}

	// 4. Stop Vane
	proc1.Stop()
	time.Sleep(1 * time.Second)

	// 5. Start Vane (Run 2) - Plugin should be loaded from disk
	// Note: We use same token just for consistency, though it doesn't affect plugin loading.
	proc2, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc2.Stop()

	// 6. Configure Listener
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	if err := writeListenerConfig(sb, vanePort, upstream.Port); err != nil {
		t.Fatal(err)
	}

	if err := proc2.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start (Run 2)", term.NewNode(err.Error())))
	}

	// 7. Verify Traffic
	if err := verifyTraffic(vanePort); err != nil {
		t.Fatal(term.FormatFailure("Traffic Verification Failed (Run 2)", term.NewNode(err.Error())))
	}
}

// Helpers

func registerPlugin(consolePort int, token, binPath string) error {
	url := fmt.Sprintf("http://127.0.0.1:%d/plugins/external.tester", consolePort)
	payload := map[string]interface{}{
		"name": "external.tester",
		"role": "middleware",
		"driver": map[string]interface{}{
			"type":    "command",
			"program": binPath,
		},
		"params": []map[string]interface{}{
			{"name": "test_val", "required": true},
		},
		"output": []string{"true", "false"},
	}
	jsonBytes, _ := json.Marshal(payload)

	req, _ := http.NewRequest("POST", url, bytes.NewBuffer(jsonBytes))
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")

	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 && resp.StatusCode != 201 {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("status %d: %s", resp.StatusCode, string(body))
	}
	return nil
}

func writeListenerConfig(s *env.Sandbox, vanePort, upstreamPort int) error {
	proxyStep := advanced.ProcessingStep{
		"internal.transport.proxy": advanced.PluginInstance{
			Input: map[string]interface{}{
				"target.ip":   "127.0.0.1",
				"target.port": upstreamPort,
			},
		},
	}

	flowConf := advanced.L4FlowConfig{
		Connection: advanced.ProcessingStep{
			"external.tester": advanced.PluginInstance{
				Input: map[string]interface{}{
					"test_val": "hello",
				},
				Output: map[string]advanced.ProcessingStep{
					"true": proxyStep,
				},
			},
		},
	}

	flowBytes, _ := json.Marshal(flowConf)
	return s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), flowBytes)
}

func verifyTraffic(port int) error {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 1*time.Second)
	if err != nil {
		return err
	}
	defer conn.Close()

	fmt.Fprintf(conn, "ping\n")
	resp, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil || resp != "ping\n" {
		return fmt.Errorf("unexpected response: %q", resp)
	}
	return nil
}
