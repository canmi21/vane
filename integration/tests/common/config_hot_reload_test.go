/* integration/tests/common/config_hot_reload_test.go */
package common

import (
	"encoding/json"
	"fmt"
	"net"
	"strings"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestConfigHotReload(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// 2. Configure a valid listener chain (Listener -> L4+ -> L7)
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// 2.1 L7 Config (httpx) - Returns the response
	l7Conf := advanced.ApplicationConfig{
		Pipeline: advanced.ProcessingStep{
			"internal.terminator.response": advanced.PluginInstance{
				Input: map[string]interface{}{
					"status": 200,
					"body":   "Valid Config",
				},
			},
		},
	}
	l7Bytes, _ := json.Marshal(l7Conf)
	if err := sb.WriteConfig("application/httpx.json", l7Bytes); err != nil {
		t.Fatal(err)
	}

	// 2.2 L4+ Config (http) - Upgrades to httpx
	l4pConf := advanced.L4FlowConfig{
		Connection: advanced.ProcessingStep{
			"internal.transport.upgrade": advanced.PluginInstance{
				Input: map[string]interface{}{"protocol": "httpx"},
			},
		},
	}
	l4pBytes, _ := json.Marshal(l4pConf)
	if err := sb.WriteConfig("resolver/http.json", l4pBytes); err != nil {
		t.Fatal(err)
	}

	// 2.3 L4 Config (Listener) - Upgrades to http
	l4Conf := advanced.L4FlowConfig{
		Connection: advanced.ProcessingStep{
			"internal.transport.upgrade": advanced.PluginInstance{
				Input: map[string]interface{}{"protocol": "http"},
			},
		},
	}
	l4Bytes, _ := json.Marshal(l4Conf)
	configPath := fmt.Sprintf("listener/[%d]/tcp.json", vanePort)

	if err := sb.WriteConfig(configPath, l4Bytes); err != nil {
		t.Fatal(err)
	}

	// 3. Wait for listener to come UP
	if err := proc.WaitForLog(fmt.Sprintf("PORT %d TCP UP", vanePort), 5*time.Second); err != nil {
		// Try generic approach
		if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
			t.Fatal(term.FormatFailure("Initial listener did not start", term.NewNode(proc.DumpLogs())))
		}
	}

	// 4. Verify connectivity (retry to allow independent resolver/application watchers to catch up)
	if err := verifyTcpResponseWithRetry(vanePort, "Valid Config", 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Initial config not working", term.NewNode(err.Error())))
	}

	// 5. Inject INVALID configuration (Broken JSON)
	brokenConfig := []byte(`{ "pipeline": { "broken": [ } } }`) // Invalid JSON
	if err := sb.WriteConfig(configPath, brokenConfig); err != nil {
		t.Fatal(err)
	}

	// 6. Wait for watcher to detect the change.
	// The live crate's watcher rescans the directory and silently keeps the last known good config.
	// We wait for the Config change signal which confirms the watcher fired.
	if err := proc.WaitForLog("Config change signal", 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Vane did not detect config change", term.NewNode(proc.DumpLogs())))
	}

	// 7. Verify listener is STILL ALIVE and serving OLD config (keep-last-known-good)
	time.Sleep(500 * time.Millisecond)

	if err := verifyTcpResponse(vanePort, "Valid Config"); err != nil {
		t.Fatal(term.FormatFailure("Listener stopped working after bad config injection", term.NewNode(err.Error())))
	}
}

func verifyTcpResponse(port int, expectedSnippet string) error {
	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return err
	}
	defer conn.Close()

	conn.SetDeadline(time.Now().Add(2 * time.Second))

	conn.Write([]byte("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"))

	buf := make([]byte, 1024)
	n, err := conn.Read(buf)
	if err != nil {
		return err
	}

	response := string(buf[:n])
	if response == "" {
		return fmt.Errorf("empty response")
	}

	if !strings.Contains(response, expectedSnippet) {
		return fmt.Errorf("response does not contain expected snippet '%s'. Got: %s", expectedSnippet, response)
	}

	return nil
}

// verifyTcpResponseWithRetry retries verifyTcpResponse until success or timeout.
// This accounts for independent config watchers needing time to reload after listener comes UP.
func verifyTcpResponseWithRetry(port int, expectedSnippet string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var lastErr error
	for time.Now().Before(deadline) {
		lastErr = verifyTcpResponse(port, expectedSnippet)
		if lastErr == nil {
			return nil
		}
		time.Sleep(200 * time.Millisecond)
	}
	return lastErr
}
