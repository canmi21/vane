/* integration/tests/l7/test_cgi_lua.go */
package l7

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestCgiLua(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 0. Check if Lua is installed
	if _, err := exec.LookPath("lua"); err != nil {
		if debug {
			term.Warn("Lua not found in PATH, skipping TestCgiLua")
		}
		// Return nil to skip gracefully, or error if strictly required.
		// For now, let's fail if missing so we know environment issues.
		return term.FormatFailure("Lua interpreter not found", term.NewNode(err.Error()))
	}

	// 1. Prepare Script
	// Read source from integration/tests/l7/cgi-bin/sample_bin.lua
	cwd, _ := os.Getwd()
	sourcePath := filepath.Join(cwd, "tests", "l7", "cgi-bin", "sample_bin.lua")
	scriptContent, err := os.ReadFile(sourcePath)
	if err != nil {
		return term.FormatFailure("Failed to read Lua script source", term.NewNode(err.Error()))
	}

	// Write to Sandbox (e.g. /tmp/vane_test_xxx/config/scripts/sample.lua)
	// Note: Vane config path is usually relative to config dir or absolute.
	// We'll put it in a known location.
	scriptPath := filepath.Join(s.RootDir, "sample.lua")
	if err := os.WriteFile(scriptPath, scriptContent, 0644); err != nil {
		return term.FormatFailure("Failed to write Lua script to sandbox", term.NewNode(err.Error()))
	}

	// 2. Setup Vane
	if err := s.GenerateCertFile("default", "localhost"); err != nil {
		return err
	}
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("tls")}
	l4Bytes, _ := json.Marshal(l4)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
	l4pBytes, _ := json.Marshal(l4p)
	s.WriteConfig("resolver/tls.json", l4pBytes)

	// L7: Execute "lua sample.lua"
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewCgiExecution(
			"lua",      // Command (Interpreter)
			scriptPath, // Script Argument
			advanced.NewSendResponse(),
			advanced.NewAbortConnection(),
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

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

	// 4. Client Request
	tr := &http.Transport{
		TLSClientConfig: &tls.Config{InsecureSkipVerify: true, ServerName: "localhost"},
	}
	client := &http.Client{Transport: tr, Timeout: 2 * time.Second}

	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/lua-test?lang=lua", vanePort)
	reqBody := "Hello from Go Client"

	var resp *http.Response
	var reqErr error
	for i := 0; i < 10; i++ {
		resp, reqErr = client.Post(targetUrl, "text/plain", strings.NewReader(reqBody))
		if reqErr == nil {
			break
		}
		time.Sleep(300 * time.Millisecond)
	}

	if reqErr != nil {
		return term.FormatFailure("Request Failed", term.NewNode(reqErr.Error()))
	}
	defer resp.Body.Close()

	// 5. Verify Response
	bodyBytes, _ := io.ReadAll(resp.Body)
	bodyStr := string(bodyBytes)

	if debug {
		term.Info(fmt.Sprintf("Received Lua CGI Body:\n%s", bodyStr))
	}

	if val := resp.Header.Get("X-CGI-Test"); val != "Vane-Lua-Script" {
		return term.FormatFailure("Missing/Wrong CGI Header", term.NewNode(fmt.Sprintf("Got: %s", val)))
	}

	if !strings.Contains(bodyStr, "Method: POST") {
		return term.FormatFailure("Wrong Method in Lua output", nil)
	}
	if !strings.Contains(bodyStr, "Query: lang=lua") {
		return term.FormatFailure("Wrong Query in Lua output", nil)
	}
	if !strings.Contains(bodyStr, "Body Content: Hello from Go Client") {
		return term.FormatFailure("Wrong Body content", nil)
	}

	return nil
}
