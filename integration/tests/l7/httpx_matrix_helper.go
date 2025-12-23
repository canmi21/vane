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

	// --- 1. Setup Upstream ---
	var upstreamPort int
	var upstreamUrl string

	switch uType {
	case UpstreamH1, UpstreamH2:
		srv, err := mock.NewHttpUpstream()
		if err != nil {
			return err
		}
		defer srv.Close()
		upstreamPort = srv.Port
		upstreamUrl = fmt.Sprintf("https://127.0.0.1:%d", upstreamPort)
	case UpstreamH3:
		srv, err := mock.NewH3Upstream()
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

	// Application Config (Common)
	// Vane's FetchUpstream "version" param: "h1", "h2", "h3"
	vaneUpstreamVer := string(uType)
	// To be strict as requested:
	if uType == UpstreamH1 {
		vaneUpstreamVer = "h1.1" // Explicit H1.1
	}

	// FIXED: Set skipVerify = true because Upstream Mock uses self-signed certs
	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			upstreamUrl,
			vaneUpstreamVer,
			true, // skip_verify
			advanced.NewSendResponse(),
			advanced.NewAbortConnection(),
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

	// Listener & Resolver Config based on Client Type
	if cType == ClientH3 {
		// UDP -> QUIC -> HTTPX
		l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("quic")}
		l4Bytes, _ := json.Marshal(l4)
		s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), l4Bytes)

		l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
		l4pBytes, _ := json.Marshal(l4p)
		s.WriteConfig("resolver/quic.json", l4pBytes)
	} else {
		// TCP -> TLS -> HTTPX
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

	// --- 4. Run Client Request ---
	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/matrix", vanePort)
	reqBody := fmt.Sprintf("Matrix-%s-%s", cType, uType)

	var resp *http.Response
	var reqErr error

	// Retry logic for Vane startup
	retryLimit := time.Now().Add(5 * time.Second)

	// Create specific clients
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
			Transport: &http.Transport{
				TLSClientConfig:   tlsConf,
				ForceAttemptHTTP2: true,
			},
			Timeout: 3 * time.Second,
		}
	case ClientH3:
		// FIXED: Explicitly set ALPN to "h3" for QUIC client
		tlsConf.NextProtos = []string{"h3"}

		// Quic-go http3 client
		rt := &http3.Transport{
			TLSClientConfig: tlsConf,
			QUICConfig:      &quic.Config{},
		}
		httpClient = &http.Client{
			Transport: rt,
			Timeout:   3 * time.Second,
		}
	}

	// Loop for request
	for time.Now().Before(retryLimit) {
		resp, reqErr = httpClient.Post(targetUrl, "text/plain", strings.NewReader(reqBody))
		if reqErr == nil {
			break
		}
		// If Vane aborts connection immediately (e.g. while logic loading), it might be retriable
		time.Sleep(300 * time.Millisecond)
	}

	if reqErr != nil {
		return term.FormatFailure("Request Failed", term.NewNode(reqErr.Error()))
	}
	defer resp.Body.Close()

	// --- 5. Verify ---
	// Protocol Check (Client Side)
	if cType == ClientH2 && resp.ProtoMajor != 2 {
		return term.FormatFailure("Client expected H2 response", term.NewNode(fmt.Sprintf("Got: %v", resp.Proto)))
	}

	// Payload Check
	body, _ := io.ReadAll(resp.Body)
	if string(body) != reqBody {
		return term.FormatFailure("Body mismatch", term.NewNode(fmt.Sprintf("Sent: %s, Got: %s", reqBody, body)))
	}

	return nil
}
