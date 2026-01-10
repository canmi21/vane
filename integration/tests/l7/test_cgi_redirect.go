/* integration/tests/l7/test_cgi_redirect.go */
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
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestCgiRedirect(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Compile the CGI binary
	binPath, err := s.CompileCgiBin("tests/l7/cgi-bin/redirect_login.c")
	if err != nil {
		return term.FormatFailure("CGI Compilation Failed", term.NewNode(err.Error()))
	}
	if debug {
		term.Info(fmt.Sprintf("Compiled CGI bin to: %s", binPath))
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

	// L7: Execute CGI that returns 302 redirect
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewCgiExecution(
			binPath,
			"",
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

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// 4. Test POST Login (should return 302)
	tr := &http.Transport{
		TLSClientConfig: &tls.Config{InsecureSkipVerify: true, ServerName: "localhost"},
	}
	client := &http.Client{
		Transport: tr,
		Timeout:   2 * time.Second,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			// Do not follow redirects - we want to inspect the 302 response
			return http.ErrUseLastResponse
		},
	}

	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/login", vanePort)
	reqBody := "username=admin&password=secret"

	var resp *http.Response
	var reqErr error
	for i := 0; i < 10; i++ {
		resp, reqErr = client.Post(targetUrl, "application/x-www-form-urlencoded", strings.NewReader(reqBody))
		if reqErr == nil {
			break
		}
		time.Sleep(300 * time.Millisecond)
	}

	if reqErr != nil {
		return term.FormatFailure("Login POST request failed", term.NewNode(reqErr.Error()))
	}
	defer resp.Body.Close()

	// 5. Verify 302 Redirect Response
	if resp.StatusCode != http.StatusFound {
		return term.FormatFailure(
			"Expected 302 Found status",
			term.NewNode(fmt.Sprintf("Got: %d %s", resp.StatusCode, resp.Status)),
		)
	}

	// 6. Verify Location Header
	location := resp.Header.Get("Location")
	if location != "/dashboard" {
		return term.FormatFailure(
			"Expected Location: /dashboard",
			term.NewNode(fmt.Sprintf("Got: %s", location)),
		)
	}

	// 7. Verify Set-Cookie Header
	cookies := resp.Cookies()
	if len(cookies) == 0 {
		return term.FormatFailure("Expected Set-Cookie header", term.NewNode("No cookies found"))
	}

	sessionFound := false
	for _, cookie := range cookies {
		if cookie.Name == "session_id" && cookie.Value == "test_session_12345" {
			sessionFound = true
			if cookie.Path != "/" {
				return term.FormatFailure(
					"Cookie path mismatch",
					term.NewNode(fmt.Sprintf("Expected '/', got '%s'", cookie.Path)),
				)
			}
			if !cookie.HttpOnly {
				return term.FormatFailure("Expected HttpOnly cookie", nil)
			}
		}
	}

	if !sessionFound {
		return term.FormatFailure(
			"Expected session_id cookie",
			term.NewNode(fmt.Sprintf("Cookies: %+v", cookies)),
		)
	}

	// 8. Read Body (302 can have body, but it's optional)
	bodyBytes, _ := io.ReadAll(resp.Body)
	if debug {
		term.Info(fmt.Sprintf("302 Response Body (%d bytes): %s", len(bodyBytes), string(bodyBytes)))
	}

	// 9. Test Invalid Login (should return 401)
	resp2, reqErr2 := client.Post(targetUrl, "application/x-www-form-urlencoded", strings.NewReader("invalid"))
	if reqErr2 != nil {
		return term.FormatFailure("Invalid login request failed", term.NewNode(reqErr2.Error()))
	}
	defer resp2.Body.Close()

	if resp2.StatusCode != http.StatusUnauthorized {
		return term.FormatFailure(
			"Expected 401 for invalid credentials",
			term.NewNode(fmt.Sprintf("Got: %d", resp2.StatusCode)),
		)
	}

	if debug {
		term.Info("✓ CGI 302 Redirect and Set-Cookie test passed")
	}

	return nil
}
