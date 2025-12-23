/* integration/tests/l7/test_https_proxy.go */
package l7

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

// TestHttpsProxy verifies the full L7 stack:
// Client (HTTPS) -> Vane (Terminator) -> Vane L7 (Upstream) -> Mock (HTTP)
func TestHttpsProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Upstream (HTTP/1.1)
	srv, err := mock.NewHttpEchoServer()
	if err != nil {
		return err
	}
	defer srv.Close()

	// 2. Generate Certificate for Vane
	if err := s.GenerateCertFile("default", "localhost"); err != nil {
		return err
	}

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// 3. Configure Vane Pipeline

	// A. L4 (TCP) -> Upgrade to TLS
	l4Config := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("tls"),
	}
	l4Bytes, _ := json.Marshal(l4Config)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// B. L4+ (TLS) -> Terminate & Upgrade to L7 (HTTPX)
	l4pConfig := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("httpx"),
	}
	l4pBytes, _ := json.Marshal(l4pConfig)
	s.WriteConfig("resolver/tls.json", l4pBytes)

	// C. L7 (Application) -> Fetch Upstream -> Send Response
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			fmt.Sprintf("http://127.0.0.1:%d", srv.Port),
			"h1",
			advanced.NewSendResponse(),    // Success Branch
			advanced.NewAbortConnection(), // Failure Branch
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

	// 4. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 5. Run Test: HTTPS Client Request (With Retry)
	tr := &http.Transport{
		TLSClientConfig: &tls.Config{
			InsecureSkipVerify: true,
			ServerName:         "localhost",                // Forces SNI
			NextProtos:         []string{"h2", "http/1.1"}, // Forces ALPN
		},
	}
	client := &http.Client{Transport: tr, Timeout: 2 * time.Second}

	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/hello-l7", vanePort)
	reqBody := "Vane L7 Test Payload"

	var resp *http.Response
	var reqErr error

	// Retry loop: Try for up to 5 seconds
	// This helps distinguish between "Vane crashed/failed immediately" and "Vane config not ready yet"
	retryDeadline := time.Now().Add(5 * time.Second)
	attempt := 0

	for time.Now().Before(retryDeadline) {
		attempt++
		resp, reqErr = client.Post(targetUrl, "text/plain", strings.NewReader(reqBody))

		if reqErr == nil {
			break // Success
		}

		// If the error is Connection Refused, Vane might be dead.
		// If Connection Reset, Vane might be rejecting/crashing.
		// We sleep a bit.
		time.Sleep(500 * time.Millisecond)
	}

	if reqErr != nil {
		return term.FormatFailure(
			fmt.Sprintf("HTTPS Request Failed after %d attempts", attempt),
			term.NewNode(reqErr.Error()),
		)
	}
	defer resp.Body.Close()

	// 6. Verify Response
	if val := resp.Header.Get("X-Mock-Server"); val != "Go-Std-Lib" {
		return term.FormatFailure("Missing Upstream Header", term.NewNode(fmt.Sprintf("Got: %s", val)))
	}

	bodyBytes, err := io.ReadAll(resp.Body)
	if err != nil {
		return term.FormatFailure("Failed to read response body", term.NewNode(err.Error()))
	}

	if string(bodyBytes) != reqBody {
		return term.FormatFailure("Body Mismatch", term.NewNode(fmt.Sprintf("Sent: %q, Got: %q", reqBody, string(bodyBytes))))
	}

	return nil
}
