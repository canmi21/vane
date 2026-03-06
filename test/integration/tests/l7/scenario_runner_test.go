/* test/integration/tests/l7/scenario_runner_test.go */

package l7

import (
	"bytes"
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
	"github.com/quic-go/quic-go"
	"github.com/quic-go/quic-go/http3"
)

// Scenario represents a single HTTP interaction to verify.
type Scenario struct {
	Name           string
	RequestHeaders map[string]string
	RequestBody    []byte
	ExpectStatus   int
	ExpectBody     []byte // If nil, assumes echo of RequestBody
}

// RunScenarios sets up Vane ONCE for the given protocol pair, then runs all scenarios.
func RunScenarios(ctx context.Context, s *env.Sandbox, cType ClientType, uType UpstreamType, scenarios []Scenario) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// --- 1. Setup Upstream ---
	var upstreamPort int
	var upstreamUrl string
	var cleanup func()

	switch uType {
	case UpstreamH1, UpstreamH2:
		srv, err := mock.NewHttpUpstreamWithHandler(mock.SmartEchoHandler)
		if err != nil {
			return err
		}
		cleanup = srv.Close
		upstreamPort = srv.Port
		upstreamUrl = fmt.Sprintf("https://127.0.0.1:%d", upstreamPort)
	case UpstreamH3:
		srv, err := mock.NewH3UpstreamWithHandler(mock.SmartEchoHandler)
		if err != nil {
			return err
		}
		cleanup = srv.Close
		upstreamPort = srv.Port
		upstreamUrl = fmt.Sprintf("https://127.0.0.1:%d", upstreamPort)
	}
	defer cleanup()

	// --- 2. Configure Vane ---
	if err := s.GenerateCertFile("default", "localhost"); err != nil {
		return err
	}
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	vaneUpstreamVer := string(uType)
	if uType == UpstreamH1 {
		vaneUpstreamVer = "h1.1"
	}

	// L7 Config (WebSocket disabled for generic tests)
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			upstreamUrl,
			vaneUpstreamVer,
			true,
			false, // WebSocket disabled
			advanced.NewSendResponse(),
			advanced.NewAbortConnection(),
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

	// L4/L4+ Config
	if cType == ClientH3 {
		l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("quic")}
		l4Bytes, _ := json.Marshal(l4)
		s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), l4Bytes)

		l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
		l4pBytes, _ := json.Marshal(l4p)
		s.WriteConfig("resolver/quic.json", l4pBytes)
	} else {
		l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("tls")}
		l4Bytes, _ := json.Marshal(l4)
		s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

		l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
		l4pBytes, _ := json.Marshal(l4p)
		s.WriteConfig("resolver/tls.json", l4pBytes)
	}

	// --- 3. Start Vane ---
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for port to be ready (H3 uses UDP, H1/H2 use TCP)
	if cType == ClientH3 {
		if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
			return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
		}
	} else {
		if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
			return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
		}
	}

	// --- 4. Prepare Client ---
	tlsConf := &tls.Config{
		InsecureSkipVerify: true,
		ServerName:         "localhost",
	}
	var httpClient *http.Client

	switch cType {
	case ClientH1:
		tlsConf.NextProtos = []string{"http/1.1"}
		httpClient = &http.Client{
			Transport: &http.Transport{TLSClientConfig: tlsConf},
			Timeout:   2 * time.Second,
		}
	case ClientH2:
		tlsConf.NextProtos = []string{"h2"}
		httpClient = &http.Client{
			Transport: &http.Transport{TLSClientConfig: tlsConf, ForceAttemptHTTP2: true},
			Timeout:   2 * time.Second,
		}
	case ClientH3:
		tlsConf.NextProtos = []string{"h3"}
		rt := &http3.Transport{
			TLSClientConfig: tlsConf,
			QUICConfig:      &quic.Config{},
		}
		httpClient = &http.Client{
			Transport: rt,
			Timeout:   2 * time.Second,
		}
	}

	// Wait for Vane to be ready (Simple probe)
	probeUrl := fmt.Sprintf("https://127.0.0.1:%d/probe", vanePort)
	for i := 0; i < 15; i++ {
		_, err := httpClient.Get(probeUrl)
		if err == nil {
			break
		}
		time.Sleep(200 * time.Millisecond)
	}

	// --- 5. Execute Scenarios ---
	baseUrl := fmt.Sprintf("https://127.0.0.1:%d", vanePort)

	for _, sc := range scenarios {
		if debug {
			term.Info(fmt.Sprintf("Running Scenario: %s", sc.Name))
		}

		reqUrl := baseUrl + "/" + strings.ReplaceAll(sc.Name, " ", "_")
		req, _ := http.NewRequest("POST", reqUrl, bytes.NewReader(sc.RequestBody))

		// Set Control Header
		req.Header.Set("X-Test-Status", fmt.Sprintf("%d", sc.ExpectStatus))

		// Set Custom Headers
		for k, v := range sc.RequestHeaders {
			req.Header.Set(k, v)
		}

		resp, err := httpClient.Do(req)
		if err != nil {
			return term.FormatFailure(fmt.Sprintf("[%s] Request Failed", sc.Name), term.NewNode(err.Error()))
		}

		// Verify Status
		if resp.StatusCode != sc.ExpectStatus {
			return term.FormatFailure(fmt.Sprintf("[%s] Status Mismatch", sc.Name),
				term.NewNode(fmt.Sprintf("Expected: %d, Got: %d", sc.ExpectStatus, resp.StatusCode)))
		}

		// Verify Body
		gotBody, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		expectBody := sc.ExpectBody
		if expectBody == nil && sc.ExpectStatus != 204 {
			expectBody = sc.RequestBody
		}

		if !bytes.Equal(gotBody, expectBody) {
			// Truncate for display if too long
			dispGot := string(gotBody)
			if len(dispGot) > 50 {
				dispGot = dispGot[:50] + "..."
			}
			return term.FormatFailure(fmt.Sprintf("[%s] Body Mismatch", sc.Name),
				term.NewNode(fmt.Sprintf("Len Expected: %d, Len Got: %d\nGot Preview: %s", len(expectBody), len(gotBody), dispGot)))
		}
	}

	return nil
}
