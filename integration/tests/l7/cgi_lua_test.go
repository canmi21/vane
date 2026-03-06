/* integration/tests/l7/cgi_lua_test.go */
package l7

import (
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestCgiLua(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 0. Check if Lua is installed
	if _, err := exec.LookPath("lua"); err != nil {
		if debug {
			term.Warn("Lua not found in PATH, skipping TestCgiLua")
		}
		// Return nil to skip gracefully, or error if strictly required.
		// For now, let's fail if missing so we know environment issues.
		t.Fatal(term.FormatFailure("Lua interpreter not found", term.NewNode(err.Error())))
	}

	// 1. Prepare Script
	// Read source from integration/tests/l7/cgi-bin/sample_bin.lua
	cwd, _ := os.Getwd()
	sourcePath := filepath.Join(cwd, "tests", "l7", "cgi-bin", "sample_bin.lua")
	scriptContent, err := os.ReadFile(sourcePath)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to read Lua script source", term.NewNode(err.Error())))
	}

	// Write to Sandbox (e.g. /tmp/vane_test_xxx/config/scripts/sample.lua)
	// Note: Vane config path is usually relative to config dir or absolute.
	// We'll put it in a known location.
	scriptPath := filepath.Join(sb.RootDir, "sample.lua")
	if err := os.WriteFile(scriptPath, scriptContent, 0644); err != nil {
		t.Fatal(term.FormatFailure("Failed to write Lua script to sandbox", term.NewNode(err.Error())))
	}

	// 2. Setup Vane
	if err := sb.GenerateCertFile("default", "localhost"); err != nil {
		t.Fatal(err)
	}
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("tls")}
	l4Bytes, _ := json.Marshal(l4)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
	l4pBytes, _ := json.Marshal(l4p)
	sb.WriteConfig("resolver/tls.json", l4pBytes)

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
	sb.WriteConfig("application/httpx.json", l7Bytes)

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
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
		t.Fatal(term.FormatFailure("Request Failed", term.NewNode(reqErr.Error())))
	}
	defer resp.Body.Close()

	// 5. Verify Response
	bodyBytes, _ := io.ReadAll(resp.Body)
	bodyStr := string(bodyBytes)

	if debug {
		term.Info(fmt.Sprintf("Received Lua CGI Body:\n%s", bodyStr))
	}

	if val := resp.Header.Get("X-CGI-Test"); val != "Vane-Lua-Script" {
		t.Fatal(term.FormatFailure("Missing/Wrong CGI Header", term.NewNode(fmt.Sprintf("Got: %s", val))))
	}

	if !strings.Contains(bodyStr, "Method: POST") {
		t.Fatal(term.FormatFailure("Wrong Method in Lua output", nil))
	}
	if !strings.Contains(bodyStr, "Query: lang=lua") {
		t.Fatal(term.FormatFailure("Wrong Query in Lua output", nil))
	}
	if !strings.Contains(bodyStr, "Body Content: Hello from Go Client") {
		t.Fatal(term.FormatFailure("Wrong Body content", nil))
	}
}
