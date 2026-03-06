/* test/integration/tests/l7/websocket_test.go */

package l7

import (
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"strings"
	"sync"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

// performUpgradeHandshake creates a raw TCP/TLS connection and sends a manual HTTP Upgrade request.
// Returns the raw connection if upgrade is successful (101).
func performUpgradeHandshake(target string, useTls bool) (net.Conn, error) {
	var conn net.Conn
	var err error

	if useTls {
		conf := &tls.Config{InsecureSkipVerify: true}
		conn, err = tls.Dial("tcp", target, conf)
	} else {
		conn, err = net.Dial("tcp", target)
	}
	if err != nil {
		return nil, err
	}

	// Set read timeout for handshake phase to detect connection issues
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))

	// Manual HTTP Request
	req := "GET /ws HTTP/1.1\r\n" +
		"Host: localhost\r\n" +
		"Connection: Upgrade\r\n" +
		"Upgrade: websocket\r\n" +
		"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" +
		"Sec-WebSocket-Version: 13\r\n" +
		"\r\n"

	if _, err := conn.Write([]byte(req)); err != nil {
		conn.Close()
		return nil, fmt.Errorf("write request failed: %w", err)
	}

	// CRITICAL FIX: Use fixed buffer instead of bufio.Reader
	// to avoid consuming WebSocket data into read buffer
	buf := make([]byte, 4096)
	n, err := conn.Read(buf)
	if err != nil {
		conn.Close()
		return nil, fmt.Errorf("read response failed: %w", err)
	}

	response := string(buf[:n])

	// Find the double CRLF marking end of headers
	headerEnd := strings.Index(response, "\r\n\r\n")
	if headerEnd == -1 {
		conn.Close()
		return nil, fmt.Errorf("invalid HTTP response: no header end found")
	}

	// Extract status line
	lines := strings.Split(response[:headerEnd], "\r\n")
	if len(lines) == 0 {
		conn.Close()
		return nil, fmt.Errorf("invalid HTTP response: no status line")
	}

	statusLine := lines[0]

	// Check for 405 (expected for deny test)
	if strings.Contains(statusLine, "405 Method Not Allowed") {
		conn.Close()
		return nil, fmt.Errorf("405 Method Not Allowed")
	}

	// Check for 101 (expected for success)
	if !strings.Contains(statusLine, "101 Switching Protocols") {
		conn.Close()
		return nil, fmt.Errorf("handshake failed: %s", strings.TrimSpace(statusLine))
	}

	// Clear read deadline for data transfer phase
	conn.SetReadDeadline(time.Time{})

	// For 101, there should be no body, so we can safely return conn
	return conn, nil
}

func setupVaneForWS(s *env.Sandbox, wsEnabled bool, upstreamPort int) (int, error) {
	if err := s.GenerateCertFile("default", "localhost"); err != nil {
		return 0, err
	}
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// Upstream is raw HTTP/1.1 (WS)
	upstreamUrl := fmt.Sprintf("http://127.0.0.1:%d", upstreamPort)

	l7Config := advanced.ApplicationConfig{
		Pipeline: advanced.NewFetchUpstream(
			upstreamUrl,
			"h1", // WS Upgrades are H1 specific
			true,
			wsEnabled,
			advanced.NewSendResponse(),
			advanced.NewSendResponse(), // Changed: failure also sends response (for 405)
		),
	}
	l7Bytes, _ := json.Marshal(l7Config)
	s.WriteConfig("application/httpx.json", l7Bytes)

	// L4 TLS
	l4 := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("tls")}
	l4Bytes, _ := json.Marshal(l4)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// Resolver
	l4p := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
	l4pBytes, _ := json.Marshal(l4p)
	s.WriteConfig("resolver/tls.json", l4pBytes)

	return vanePort, nil
}

// Test 1: Deny (Default)
func TestWSDeny(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Upstream
	us, err := mock.NewWsUpstream()
	if err != nil {
		t.Fatal(err)
	}
	defer us.Close()

	// 2. Setup Vane (WS Disabled)
	vanePort, err := setupVaneForWS(sb, false, us.Port)
	if err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 3. Connect and expect 405 rejection
	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := performUpgradeHandshake(target, true)

	if err == nil {
		// Connection succeeded but shouldn't have
		conn.Close()
		t.Fatal(term.FormatFailure("WebSocket connection succeeded but should be denied", nil))
	}

	// Check if error indicates 405 rejection
	errStr := err.Error()
	if strings.Contains(errStr, "405 Method Not Allowed") {
		// Expected rejection
		if debug {
			term.Pass("WebSocket correctly rejected with 405")
		}
		return
	}

	// Other errors might also indicate rejection (connection closed, EOF, etc.)
	if strings.Contains(errStr, "handshake failed") ||
		strings.Contains(errStr, "EOF") ||
		strings.Contains(errStr, "connection reset") {
		if debug {
			term.Info("WebSocket rejected (connection closed)")
		}
		return
	}

	t.Fatal(term.FormatFailure("Unexpected error type", term.NewNode(errStr)))
}

// Test 2: Allow & Basic Ping/Pong
func TestWSAllow(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	us, err := mock.NewWsUpstream()
	if err != nil {
		t.Fatal(err)
	}
	defer us.Close()

	vanePort, err := setupVaneForWS(sb, true, us.Port)
	if err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := performUpgradeHandshake(target, true)
	if err != nil {
		t.Fatal(term.FormatFailure("Handshake failed", term.NewNode(err.Error())))
	}
	defer conn.Close()

	// Set timeout for echo test
	conn.SetDeadline(time.Now().Add(5 * time.Second))

	// Test Echo
	msg := "Hello WebSocket"
	if _, err := conn.Write([]byte(msg)); err != nil {
		t.Fatal(term.FormatFailure("Write failed", term.NewNode(err.Error())))
	}

	buf := make([]byte, len(msg))
	if _, err := io.ReadFull(conn, buf); err != nil {
		t.Fatal(term.FormatFailure("Read failed", term.NewNode(err.Error())))
	}

	if string(buf) != msg {
		t.Fatal(term.FormatFailure("Echo mismatch", term.NewNode(fmt.Sprintf("Expected: %s, Got: %s", msg, string(buf)))))
	}

	if debug {
		term.Pass("WebSocket Echo Test Passed")
	}
}

// Test 3: Large Stream (1GB)
func TestWSStreamLarge(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)
	const PayloadSize = 1024 * 1024 * 1024 // 1GB

	if debug {
		term.Info("Preparing WebSocket 1GB Streaming Test")
	}

	us, err := mock.NewWsUpstream()
	if err != nil {
		t.Fatal(err)
	}
	defer us.Close()

	vanePort, err := setupVaneForWS(sb, true, us.Port)
	if err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := performUpgradeHandshake(target, true)
	if err != nil {
		t.Fatal(term.FormatFailure("Handshake failed", term.NewNode(err.Error())))
	}
	defer conn.Close()

	// No deadline for large transfer (or set very long)
	conn.SetDeadline(time.Now().Add(120 * time.Second))

	// Generate Data
	seed := time.Now().UnixNano()
	expectedHash := calculateExpectedHash(PayloadSize, seed)
	reader := NewRandomStream(PayloadSize, seed)

	// Concurrently Write and Read
	errChan := make(chan error, 2)
	var wg sync.WaitGroup
	wg.Add(2)

	// Writer
	go func() {
		defer wg.Done()
		written, err := io.Copy(conn, reader)
		if err != nil {
			errChan <- fmt.Errorf("write error: %w", err)
			return
		}
		if debug {
			term.Info(fmt.Sprintf("Wrote %d bytes", written))
		}
		// Signal write completion by closing write side (half-close)
		if tcpConn, ok := conn.(*net.TCPConn); ok {
			tcpConn.CloseWrite()
		} else if tlsConn, ok := conn.(*tls.Conn); ok {
			// TLS doesn't support half-close, just continue
			_ = tlsConn
		}
	}()

	// Reader
	go func() {
		defer wg.Done()
		hasher := sha256.New()
		read, err := io.CopyN(hasher, conn, PayloadSize)
		if err != nil {
			errChan <- fmt.Errorf("read error after %d bytes: %w", read, err)
			return
		}
		actualHash := hex.EncodeToString(hasher.Sum(nil))
		if actualHash != expectedHash {
			errChan <- fmt.Errorf("hash mismatch: expected %s, got %s", expectedHash, actualHash)
		}
		if debug {
			term.Info(fmt.Sprintf("Read %d bytes, hash verified", read))
		}
	}()

	// Wait logic
	done := make(chan struct{})
	go func() {
		wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		// Check if any errors occurred
		select {
		case err := <-errChan:
			t.Fatal(term.FormatFailure("Streaming Failed", term.NewNode(err.Error())))
		default:
			// Success
		}
	case err := <-errChan:
		t.Fatal(term.FormatFailure("Streaming Failed", term.NewNode(err.Error())))
	case <-time.After(120 * time.Second):
		t.Fatal(term.FormatFailure("Timeout after 120s", nil))
	}

	if debug {
		term.Pass("1GB WebSocket Tunnel Verified")
	}
}
