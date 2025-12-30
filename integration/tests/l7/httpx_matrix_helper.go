/* integration/tests/l7/httpx_matrix_helper.go */
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
	"github.com/quic-go/quic-go"
	"github.com/quic-go/quic-go/http3"
)

type UpstreamType string

const (
	UpstreamH1 UpstreamType = "h1"
	UpstreamH2 UpstreamType = "h2"
	UpstreamH3 UpstreamType = "h3"
)

type ClientType string

const (
	ClientH1 ClientType = "h1"
	ClientH2 ClientType = "h2"
	ClientH3 ClientType = "h3"
)

// RunMatrixTest handles the complexity of configuring Vane and the Clients/Servers.
func RunMatrixTest(ctx context.Context, s *env.Sandbox, cType ClientType, uType UpstreamType) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// --- 1. Setup Upstream with Header Echo Logic ---
	// We need a handler that echoes a specific header to verify propagation.
	handler := func(w http.ResponseWriter, r *http.Request) {
		// Verify Request Header
		clientHeader := r.Header.Get("X-Client-Token")
		if clientHeader != "vane-matrix-test" {
			w.WriteHeader(http.StatusBadRequest)
			w.Write([]byte("Missing or wrong X-Client-Token header"))
			return
		}

		// Set Response Header
		w.Header().Set("X-Upstream-Response", "confirmed")
		w.Header().Set("X-Upstream-Proto", r.Proto) // e.g. "HTTP/1.1", "HTTP/2.0" or "HTTP/3.0"

		// Echo Body
		body, _ := io.ReadAll(r.Body)
		w.Write(body)
	}

	var upstreamPort int
	var upstreamUrl string

	switch uType {
	case UpstreamH1, UpstreamH2:
		srv, err := mock.NewHttpUpstreamWithHandler(handler)
		if err != nil {
			return err
		}
		defer srv.Close()
		upstreamPort = srv.Port
		upstreamUrl = fmt.Sprintf("https://127.0.0.1:%d", upstreamPort)
	case UpstreamH3:
		srv, err := mock.NewH3UpstreamWithHandler(handler)
		if err != nil {
			return err
		}
		defer srv.Close()
		upstreamPort = srv.Port
		upstreamUrl = fmt.Sprintf("https://127.0.0.1:%d", upstreamPort)
	}

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

	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			upstreamUrl,
			vaneUpstreamVer,
			true, // skip_verify
			false,
			advanced.NewSendResponse(),
			advanced.NewAbortConnection(),
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

	// Listener & Resolver Config
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

	// --- 4. Run Client Request ---
	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/matrix", vanePort)
	reqBody := fmt.Sprintf("Matrix-%s-%s", cType, uType)

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
			Timeout:   3 * time.Second,
		}
	case ClientH2:
		tlsConf.NextProtos = []string{"h2"}
		httpClient = &http.Client{
			Transport: &http.Transport{TLSClientConfig: tlsConf, ForceAttemptHTTP2: true},
			Timeout:   3 * time.Second,
		}
	case ClientH3:
		tlsConf.NextProtos = []string{"h3"}
		rt := &http3.Transport{
			TLSClientConfig: tlsConf,
			QUICConfig:      &quic.Config{},
		}
		httpClient = &http.Client{
			Transport: rt,
			Timeout:   3 * time.Second,
		}
	}

	var resp *http.Response
	var reqErr error
	retryLimit := time.Now().Add(5 * time.Second)

	for time.Now().Before(retryLimit) {
		// Construct Request with Custom Header
		req, _ := http.NewRequest("POST", targetUrl, strings.NewReader(reqBody))
		req.Header.Set("X-Client-Token", "vane-matrix-test")
		req.Header.Set("Content-Type", "text/plain")

		resp, reqErr = httpClient.Do(req)
		if reqErr == nil {
			break
		}
		time.Sleep(300 * time.Millisecond)
	}

	if reqErr != nil {
		return term.FormatFailure("Request Failed", term.NewNode(reqErr.Error()))
	}
	defer resp.Body.Close()

	// --- 5. Verify ---
	// A. Status Code (Proof that request reached Upstream logic)
	if resp.StatusCode != 200 {
		body, _ := io.ReadAll(resp.Body)
		return term.FormatFailure("Upstream rejected request",
			term.NewNode(fmt.Sprintf("Status: %d, Body: %s", resp.StatusCode, string(body))))
	}

	// B. Header Propagation (Upstream -> Client)
	if val := resp.Header.Get("X-Upstream-Response"); val != "confirmed" {
		return term.FormatFailure("Header propagation failed (Resp)",
			term.NewNode(fmt.Sprintf("Expected 'confirmed', got '%s'", val)))
	}

	// C. Protocol Check
	if cType == ClientH2 && resp.ProtoMajor != 2 {
		return term.FormatFailure("Client expected H2 response", term.NewNode(fmt.Sprintf("Got: %v", resp.Proto)))
	}

	// D. Payload Check
	body, _ := io.ReadAll(resp.Body)
	if string(body) != reqBody {
		return term.FormatFailure("Body mismatch", term.NewNode(fmt.Sprintf("Sent: %s, Got: %s", reqBody, body)))
	}

	return nil
}
