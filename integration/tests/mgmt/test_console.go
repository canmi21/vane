/* integration/tests/mgmt/test_console.go */
package mgmt

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"regexp"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

// TestConsoleHttp verifies that the management API is accessible via HTTP.
func TestConsoleHttp(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane with a known token

	token := "test-console-http-token"
	s.Env["ACCESS_TOKEN"] = token
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 2. Request the root endpoint

	url := fmt.Sprintf("http://127.0.0.1:%d/", s.ConsolePort)
	req, _ := http.NewRequest("GET", url, nil)
	req.Header.Set("Authorization", "Bearer "+token)

	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return term.FormatFailure("HTTP Request Failed", term.NewNode(err.Error()))
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return term.FormatFailure(fmt.Sprintf("Unexpected Status: %d", resp.StatusCode), nil)
	}

	var result map[string]interface{}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return term.FormatFailure("Failed to decode JSON", term.NewNode(err.Error()))
	}

	if result["status"] != "success" {
		return term.FormatFailure(fmt.Sprintf("Expected status 'success', got '%v'", result["status"]), nil)
	}

	return nil
}

// TestConsoleUds verifies that the management API is accessible via Unix Domain Socket.
func TestConsoleUds(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane

	token := "test-console-uds-token"
	s.Env["ACCESS_TOKEN"] = token
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 2. Extract UDS path from logs
	// Log pattern: "✓ Management console listening on unix:/tmp/..."
	re := regexp.MustCompile(`Management console listening on unix:([^✓ ]+)`)
	var udsPath string

	// Wait up to 3 seconds for the log to appear

	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		logs := proc.DumpLogs()
		matches := re.FindStringSubmatch(logs)
		if len(matches) > 1 {
			udsPath = strings.TrimSpace(matches[1])
			break
		}

		time.Sleep(200 * time.Millisecond)
	}

	if udsPath == "" {
		return term.FormatFailure("Could not find UDS path in logs", term.NewNode(proc.DumpLogs()))
	}

	// 3. Connect via UDS and send raw HTTP GET
	conn, err := net.DialTimeout("unix", udsPath, 1*time.Second)
	if err != nil {
		return term.FormatFailure("Failed to connect to UDS", term.NewNode(err.Error()))
	}
	defer conn.Close()

	request := fmt.Sprintf("GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer %s\r\nConnection: close\r\n\r\n", token)
	if _, err := conn.Write([]byte(request)); err != nil {
		return term.FormatFailure("Failed to write to UDS", term.NewNode(err.Error()))
	}

	// 4. Parse response
	resp, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		return term.FormatFailure("Failed to read HTTP response from UDS", term.NewNode(err.Error()))
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return term.FormatFailure(fmt.Sprintf("Unexpected UDS HTTP Status: %d", resp.StatusCode), nil)
	}

	var result map[string]interface{}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return term.FormatFailure("Failed to decode UDS JSON", term.NewNode(err.Error()))
	}

	if result["status"] != "success" {
		return term.FormatFailure(fmt.Sprintf("Expected status 'success', got '%v'", result["status"]), nil)
	}

	return nil
}

// TestConsoleNoToken verifies that the management API is DISABLED when no token is provided.
func TestConsoleNoToken(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane without token

	proc, err := s.StartVaneWithoutToken(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 2. Verify console port is NOT listening
	if err := proc.WaitForNoConsole(2 * time.Second); err != nil {
		return term.FormatFailure("Console should be disabled", term.NewNode(err.Error()))
	}

	return nil
}
