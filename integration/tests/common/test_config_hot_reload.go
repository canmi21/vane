/* integration/tests/common/test_config_hot_reload.go */
package common

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestConfigHotReload(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
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
	if err := s.WriteConfig("application/httpx.json", l7Bytes); err != nil {
		return err
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
	if err := s.WriteConfig("resolver/http.json", l4pBytes); err != nil {
		return err
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

	if err := s.WriteConfig(configPath, l4Bytes); err != nil {
		return err
	}

	// 3. Wait for listener to come UP
	if err := proc.WaitForLog(fmt.Sprintf("PORT %d TCP UP", vanePort), 5*time.Second); err != nil {
		// Try generic approach
		if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
			return term.FormatFailure("Initial listener did not start", term.NewNode(proc.DumpLogs()))
		}
	}

	// 4. Verify connectivity
	if err := verifyTcpResponse(vanePort, "Valid Config"); err != nil {
		return term.FormatFailure("Initial config not working", term.NewNode(err.Error()))
	}

	// 5. Inject INVALID configuration (Broken JSON)
	// We deliberately break the JSON syntax

	brokenConfig := []byte(`{ "pipeline": { "broken": [ } } }`) // Invalid JSON
	if err := s.WriteConfig(configPath, brokenConfig); err != nil {
		return err
	}

	// 6. Wait for "Keep Last Known Good" Log
	// Log message from source: "New TCP config for port ... is invalid. Keeping last known good version."
	expectedLog := fmt.Sprintf("New TCP config for port %d is invalid. Keeping last known good version", vanePort)
	if err := proc.WaitForLog(expectedLog, 5*time.Second); err != nil {
		return term.FormatFailure("Vane did not report keeping last known good config", term.NewNode(proc.DumpLogs()))
	}

	// 7. Verify listener is STILL ALIVE and serving OLD config
	// Give it a moment to ensure it didn't crash or close the socket
	time.Sleep(1 * time.Second)

	if err := verifyTcpResponse(vanePort, "Valid Config"); err != nil {
		return term.FormatFailure("Listener stopped working after bad config injection", term.NewNode(err.Error()))
	}

	return nil
}

func verifyTcpResponse(port int, expectedSnippet string) error {
	conn, err := net.Dial("tcp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return err
	}
	defer conn.Close()

	// Set deadline
	conn.SetDeadline(time.Now().Add(2 * time.Second))

	// Send a valid HTTP request because we upgraded to httpx
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
