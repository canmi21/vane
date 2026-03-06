/* integration/tests/l7/https_proxy_test.go */
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
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

// TestHttpsProxy verifies the full L7 stack:
// Client (HTTPS) -> Vane (Terminator) -> Vane L7 (Upstream) -> Mock (HTTP)
func TestHttpsProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Upstream (HTTP/1.1)
	srv, err := mock.NewHttpEchoServer()
	if err != nil {
		t.Fatal(err)
	}
	defer srv.Close()

	// 2. Generate Certificate for Vane
	if err := sb.GenerateCertFile("default", "localhost"); err != nil {
		t.Fatal(err)
	}

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// 3. Configure Vane Pipeline

	// A. L4 (TCP) -> Upgrade to TLS
	l4Config := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Config)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// B. L4+ (TLS) -> Terminate & Upgrade to L7 (HTTPX)
	l4pConfig := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("httpx"),
	}
	l4pBytes, _ := json.Marshal(l4pConfig)
	sb.WriteConfig("resolver/tls.json", l4pBytes)

	// C. L7 (Application) -> Fetch Upstream -> Send Response
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			fmt.Sprintf("http://127.0.0.1:%d", srv.Port),
			"h1",
			true, // FIXED: Skip verify (although standard mock uses HTTP so it doesn't matter much, but good practice)
			false,
			advanced.NewSendResponse(),
			advanced.NewAbortConnection(),
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	sb.WriteConfig("application/httpx.json", l7Bytes)

	// 4. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 5. Run Test: HTTPS Client Request (With Retry)
	tr := &http.Transport{
		TLSClientConfig: &tls.Config{
			InsecureSkipVerify: true,
			ServerName:         "localhost",
			NextProtos:         []string{"h2", "http/1.1"},
		},
	}
	client := &http.Client{Transport: tr, Timeout: 2 * time.Second}

	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/hello-l7", vanePort)
	reqBody := "Vane L7 Test Payload"

	var resp *http.Response
	var reqErr error

	// Retry loop
	retryDeadline := time.Now().Add(5 * time.Second)
	attempt := 0

	for time.Now().Before(retryDeadline) {
		attempt++
		resp, reqErr = client.Post(targetUrl, "text/plain", strings.NewReader(reqBody))

		if reqErr == nil {
			break
		}
		if debug {
			term.Warn(fmt.Sprintf("Attempt #%d failed: %v. Retrying...", attempt, reqErr))
		}
		time.Sleep(500 * time.Millisecond)
	}

	if reqErr != nil {
		t.Fatal(term.FormatFailure(
			fmt.Sprintf("HTTPS Request Failed after %d attempts", attempt),
			term.NewNode(reqErr.Error()),
		))
	}
	defer resp.Body.Close()

	// 6. Verify Response
	if val := resp.Header.Get("X-Mock-Server"); val != "Go-Std-Lib" {
		t.Fatal(term.FormatFailure("Missing Upstream Header", term.NewNode(fmt.Sprintf("Got: %s", val))))
	}

	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to read response body", term.NewNode(err.Error())))
	}

	if string(bodyBytes) != reqBody {
		t.Fatal(term.FormatFailure("Body Mismatch", term.NewNode(fmt.Sprintf("Sent: %q, Got: %q", reqBody, string(bodyBytes)))))
	}
}
