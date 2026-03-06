/* integration/tests/l4/template_parsing_test.go */
package l4

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTemplateParsing(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Compile Validator Plugin
	// Vane requires external commands to be in the trusted 'bin' directory inside config.
	binDir := filepath.Join(sb.ConfigDir, "bin")
	if err := os.MkdirAll(binDir, 0755); err != nil {
		t.Fatal(fmt.Errorf("failed to create bin dir: %v", err))
	}

	validatorSrc := "tests/l4/assets/validator.go"
	validatorBin := filepath.Join(binDir, "validator_plugin")

	cmd := exec.Command("go", "build", "-o", validatorBin, validatorSrc)
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatal(fmt.Errorf("failed to compile validator plugin: %v\nOutput: %s", err, string(out)))
	}

	// 2. Set Access Token for API
	token := "test-token-long-12345678"
	sb.Env["ACCESS_TOKEN"] = token

	// 3. Start Vane (Before writing listener config, so we can register plugin first)
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
	}
	vanePort := ports[0]

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// 4. Register Plugin via API
	resultFile := filepath.Join(sb.RootDir, "validator_result.txt")
	apiURL := fmt.Sprintf("http://127.0.0.1:%d/plugins/test.validator", sb.ConsolePort)
	pluginConfig := map[string]interface{}{
		"name": "test.validator",
		"role": "middleware",
		"driver": map[string]interface{}{
			"type":    "command",
			"program": validatorBin,
			"args":    []string{},
			"env": map[string]string{
				"VALIDATOR_OUTPUT_FILE": resultFile,
			},
		},
		"params": []map[string]interface{}{
			{"name": "conn_ip", "required": true, "param_type": "string"},
			{"name": "conn_port", "required": true, "param_type": "string"},
			{"name": "conn_proto", "required": true, "param_type": "string"},
			{"name": "conn_uuid", "required": true, "param_type": "string"},
			{"name": "conn_timestamp", "required": true, "param_type": "string"},
			{"name": "server_ip", "required": true, "param_type": "string"},
			{"name": "server_port", "required": true, "param_type": "string"},
		},
		"output": []string{"success", "failure"},
	}
	pluginBytes, _ := json.Marshal(pluginConfig)

	req, err := http.NewRequest("POST", apiURL, bytes.NewBuffer(pluginBytes))
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+token)

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		t.Fatal(fmt.Errorf("failed to call register plugin API: %v", err))
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(resp.Body)
		t.Fatal(fmt.Errorf("API returned status %d: %s", resp.StatusCode, string(body)))
	}

	// 5. Write Flow Config (Listener) - triggers hot reload
	flowConf := map[string]interface{}{
		"connection": map[string]interface{}{
			"test.validator": map[string]interface{}{
				"input": map[string]interface{}{
					"conn_ip":        "{{conn.ip}}",
					"conn_port":      "{{conn.port}}",
					"conn_proto":     "{{conn.proto}}",
					"conn_uuid":      "{{conn.uuid}}",
					"conn_timestamp": "{{conn.timestamp}}",
					"server_ip":      "{{server.ip}}",
					"server_port":    "{{server.port}}",
				},
				"output": map[string]interface{}{
					"success": map[string]interface{}{
						"internal.transport.abort": map[string]interface{}{
							"input": map[string]interface{}{},
						},
					},
					"failure": map[string]interface{}{
						"internal.transport.abort": map[string]interface{}{
							"input": map[string]interface{}{},
						},
					},
				},
			},
		},
	}

	flowBytes, err := json.Marshal(flowConf)
	if err != nil {
		t.Fatal(err)
	}
	// Write config to trigger reload
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), flowBytes); err != nil {
		t.Fatal(err)
	}

	// Wait for listener to be UP
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start after config reload", term.NewNode(err.Error())))
	}

	// 6. Trigger Connection
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to dial Vane", term.NewNode(err.Error())))
	}
	// Send some data to ensure connection is active
	fmt.Fprintf(conn, "keepalive")
	// Wait a bit to let Vane process the flow before closing
	time.Sleep(200 * time.Millisecond)
	conn.Close()

	// 7. Verify Result via File
	timeout := 5 * time.Second
	deadline := time.Now().Add(timeout)
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			content, err := os.ReadFile(resultFile)
			if err == nil {
				sContent := string(content)
				if sContent == "SUCCESS" {
					return
				}
				if strings.HasPrefix(sContent, "FAILURE") {
					t.Fatal(term.FormatFailure(fmt.Sprintf("Validator reported failure: %s", sContent), nil))
				}
			}
		case <-time.After(time.Until(deadline)):
			t.Fatal(term.FormatFailure("Timeout waiting for validator result file", nil))
		}
	}
}
