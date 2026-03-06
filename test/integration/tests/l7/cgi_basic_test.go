/* test/integration/tests/l7/cgi_basic_test.go */

package l7

import (
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestCgiBasic(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Compile the CGI binary
	binPath, err := sb.CompileCgiBin("cgi-bin/sample_bin.c")
	if err != nil {
		t.Fatal(term.FormatFailure("CGI Compilation Failed", term.NewNode(err.Error())))
	}
	if debug {
		term.Info(fmt.Sprintf("Compiled CGI bin to: %s", binPath))
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

	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewCgiExecution(
			binPath,
			"",
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

	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/cgi-test?foo=bar", vanePort)
	reqBody := "Hello CGI World"

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
		term.Info(fmt.Sprintf("Received CGI Body (%d bytes):%s", len(bodyBytes), bodyStr))
	}

	if val := resp.Header.Get("X-CGI-Test"); val != "Vane-C-Bin" {
		t.Fatal(term.FormatFailure("Missing CGI Header", term.NewNode(fmt.Sprintf("Got: %s", val))))
	}

	if len(bodyBytes) == 0 {
		t.Fatal(term.FormatFailure("Received Empty Body", nil))
	}

	if !strings.Contains(bodyStr, "Method: POST") {
		t.Fatal(term.FormatFailure("Wrong Method in CGI output", term.NewNode(fmt.Sprintf("Full Body: %q", bodyStr))))
	}
	if !strings.Contains(bodyStr, "Query: foo=bar") {
		t.Fatal(term.FormatFailure("Wrong Query in CGI output", term.NewNode(fmt.Sprintf("Full Body: %q", bodyStr))))
	}
	if !strings.Contains(bodyStr, "Body Content: Hello CGI World") {
		t.Fatal(term.FormatFailure("Wrong Body in CGI output", term.NewNode(fmt.Sprintf("Full Body: %q", bodyStr))))
	}
}
