/* integration/tests/l7/test_streaming_large.go */
package l7

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"sync/atomic"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"github.com/quic-go/quic-go"
	"github.com/quic-go/quic-go/http3"
)

// RandomStream implements io.Reader to generate deterministic random data on the fly.
// This avoids holding 1GB in memory.
type RandomStream struct {
	Size      int64
	readCount int64
	seed      int64
	rnd       *rand.Rand
}

func NewRandomStream(size int64, seed int64) *RandomStream {
	return &RandomStream{
		Size: size,
		seed: seed,
		rnd:  rand.New(rand.NewSource(seed)),
	}
}

func (r *RandomStream) Read(p []byte) (n int, err error) {
	if r.readCount >= r.Size {
		return 0, io.EOF
	}

	left := r.Size - r.readCount
	toRead := int64(len(p))
	if toRead > left {
		toRead = left
	}

	// We use math/rand to fill buffer. It's fast enough for tests.
	n, err = r.rnd.Read(p[:toRead])
	r.readCount += int64(n)
	return n, nil
}

// CountingReader wraps an io.Reader to count bytes transferred atomically.
type CountingReader struct {
	delegate io.Reader
	count    *int64
}

func (c *CountingReader) Read(p []byte) (n int, err error) {
	n, err = c.delegate.Read(p)
	if n > 0 {
		atomic.AddInt64(c.count, int64(n))
	}
	return
}

// Calculate expected hash without storing data
func calculateExpectedHash(size int64, seed int64) string {
	r := NewRandomStream(size, seed)
	h := sha256.New()
	io.Copy(h, r)
	return hex.EncodeToString(h.Sum(nil))
}

// RunStreamingTest sets up a Vane proxy and transfers a large payload.
// cType: Client Protocol (Sender)
// uType: Upstream Protocol (Receiver/Echoer)
func RunStreamingTest(ctx context.Context, s *env.Sandbox, cType ClientType, uType UpstreamType) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	const PayloadSize = 1024 * 1024 * 1024 // 1GB

	if debug {
		term.Info(fmt.Sprintf("Preparing %s -> %s Streaming Test (Size: 1GB)", cType, uType))
	}

	// --- 1. Setup Upstream (Echo Server) ---
	var upstreamPort int
	var upstreamUrl string
	var cleanup func()

	// Use SmartEchoHandler which uses io.Copy (Streaming Echo)
	switch uType {
	case UpstreamH2:
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

	// L7: Fetch Upstream
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

	// L4/L4+ Setup
	if cType == ClientH3 {
		l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("quic")}
		l4Bytes, _ := json.Marshal(l4)
		s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), l4Bytes)

		l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
		l4pBytes, _ := json.Marshal(l4p)
		s.WriteConfig("resolver/quic.json", l4pBytes)
	} else {
		// H2 Client
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

	// Wait for port to be ready (H3 uses UDP, H2 uses TCP)
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

	// Use long timeout for 1GB transfer
	transferTimeout := 60 * time.Second

	switch cType {
	case ClientH2:
		tlsConf.NextProtos = []string{"h2"}
		httpClient = &http.Client{
			Transport: &http.Transport{TLSClientConfig: tlsConf, ForceAttemptHTTP2: true},
			Timeout:   transferTimeout,
		}
	case ClientH3:
		tlsConf.NextProtos = []string{"h3"}
		rt := &http3.Transport{
			TLSClientConfig: tlsConf,
			QUICConfig:      &quic.Config{},
		}
		httpClient = &http.Client{
			Transport: rt,
			Timeout:   transferTimeout,
		}
	}

	// Retry loop for startup
	targetUrl := fmt.Sprintf("https://127.0.0.1:%d/stream_1gb", vanePort)
	ready := false
	for i := 0; i < 10; i++ {
		// Use a small probe first
		resp, err := httpClient.Get(fmt.Sprintf("https://127.0.0.1:%d/probe", vanePort))
		if err == nil {
			resp.Body.Close()
			ready = true
			break
		}
		time.Sleep(500 * time.Millisecond)
	}
	if !ready {
		return term.FormatFailure("Vane failed to start or accept connections", nil)
	}

	// --- 5. Execute 1GB Transfer with Monitoring ---
	seed := time.Now().UnixNano()

	if debug {
		term.Info("Calculating expected hash...")
	}
	expectedHash := calculateExpectedHash(PayloadSize, seed)

	if debug {
		term.Info("Starting 1GB Upload & Download...")
	}

	// Counters for monitoring
	var uploadedBytes int64
	var downloadedBytes int64
	doneChan := make(chan bool)

	// Start Monitor Goroutine (Only in Debug mode)
	if debug {
		go func() {
			ticker := time.NewTicker(3 * time.Second)
			defer ticker.Stop()

			var lastUp, lastDown int64
			lastTime := time.Now()

			for {
				select {
				case <-doneChan:
					return
				case <-ticker.C:
					currUp := atomic.LoadInt64(&uploadedBytes)
					currDown := atomic.LoadInt64(&downloadedBytes)
					now := time.Now()

					deltaUp := float64(currUp - lastUp)
					deltaDown := float64(currDown - lastDown)
					dur := now.Sub(lastTime).Seconds()

					// Mbps = bytes * 8 / 1e6
					mbpsUp := (deltaUp * 8 / 1e6) / dur
					mbpsDown := (deltaDown * 8 / 1e6) / dur

					// Calculate percentages
					pctUp := float64(currUp) / float64(PayloadSize) * 100
					pctDown := float64(currDown) / float64(PayloadSize) * 100

					term.Info(fmt.Sprintf(
						"Progress: [Up: %.1f%% (%.2f Mbps)] [Down: %.1f%% (%.2f Mbps)]",
						pctUp, mbpsUp, pctDown, mbpsDown,
					))

					lastUp = currUp
					lastDown = currDown
					lastTime = now
				}
			}
		}()
	}

	start := time.Now()

	// Prepare wrapped Request Body
	rawStream := NewRandomStream(PayloadSize, seed)
	reqBody := &CountingReader{delegate: rawStream, count: &uploadedBytes}

	req, _ := http.NewRequest("POST", targetUrl, reqBody)
	req.Header.Set("Content-Type", "application/octet-stream")

	// Execute Request (Upload starts here)
	resp, err := httpClient.Do(req)
	if err != nil {
		if debug {
			close(doneChan)
		}
		return term.FormatFailure("1GB Transfer Failed (Request Error)", term.NewNode(err.Error()))
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		if debug {
			close(doneChan)
		}
		return term.FormatFailure("1GB Transfer Failed (Status Error)", term.NewNode(fmt.Sprintf("Status: %d", resp.StatusCode)))
	}

	// Prepare wrapped Response Body (Download starts here)
	respBody := &CountingReader{delegate: resp.Body, count: &downloadedBytes}

	// Stream Verification
	hasher := sha256.New()
	written, err := io.Copy(hasher, respBody)

	// Stop monitor
	if debug {
		close(doneChan)
	}

	if err != nil {
		return term.FormatFailure("1GB Transfer Failed (Read Error)", term.NewNode(err.Error()))
	}

	duration := time.Since(start)
	actualHash := hex.EncodeToString(hasher.Sum(nil))

	if written != PayloadSize {
		return term.FormatFailure("Size Mismatch", term.NewNode(fmt.Sprintf("Expected: %d bytes, Got: %d bytes", PayloadSize, written)))
	}

	if actualHash != expectedHash {
		return term.FormatFailure("Hash Mismatch", term.NewNode(fmt.Sprintf("Expected: %s\nGot: %s", expectedHash, actualHash)))
	}

	if debug {
		mbps := (float64(PayloadSize) * 8 / 1000 / 1000) / duration.Seconds()
		term.Pass(fmt.Sprintf("1GB Transfer Complete: %.2f Mbps (Duration: %v)", mbps, duration))
	}

	return nil
}

func TestStreamH2toH3(ctx context.Context, s *env.Sandbox) error {
	return RunStreamingTest(ctx, s, ClientH2, UpstreamH3)
}

func TestStreamH3toH2(ctx context.Context, s *env.Sandbox) error {
	return RunStreamingTest(ctx, s, ClientH3, UpstreamH2)
}
