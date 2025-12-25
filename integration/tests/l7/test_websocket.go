/* integration/tests/l7/test_websocket.go */
package l7

import (
	"bufio"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"strings"
	"sync"
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

	// Manual HTTP Request
	req := "GET /ws HTTP/1.1\r\n" +
		"Host: localhost\r\n" +
		"Connection: Upgrade\r\n" +
		"Upgrade: websocket\r\n" +
		"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" +
		"Sec-WebSocket-Version: 13\r\n" +
		"\r\n"

	conn.Write([]byte(req))

	// Read Response Header
	reader := bufio.NewReader(conn)
	statusLine, err := reader.ReadString('\n')
	if err != nil {
		conn.Close()
		return nil, err
	}

	if !strings.Contains(statusLine, "101 Switching Protocols") {
		// Read fully to debug (e.g. 405)
		conn.Close()
		return nil, fmt.Errorf("handshake failed: %s", strings.TrimSpace(statusLine))
	}

	// Consume headers until empty line
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			conn.Close()
			return nil, err
		}
		if line == "\r\n" {
			break
		}
	}

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
			advanced.NewAbortConnection(),
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
func TestWSDeny(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Upstream
	us, err := mock.NewWsUpstream()
	if err != nil {
		return err
	}
	defer us.Close()

	// 2. Setup Vane (WS Disabled)
	vanePort, err := setupVaneForWS(s, false, us.Port)
	if err != nil {
		return err
	}

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 3. Connect
	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	_, err = performUpgradeHandshake(target, true)

	// Expect Failure (e.g. 405)
	if err == nil {
		return term.FormatFailure("WS Connection succeeded but should be denied", nil)
	}
	if !strings.Contains(err.Error(), "405 Method Not Allowed") {
		return term.FormatFailure("Expected 405 error", term.NewNode(err.Error()))
	}

	return nil
}

// Test 2: Allow & Basic Ping/Pong
func TestWSAllow(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	us, err := mock.NewWsUpstream()
	if err != nil {
		return err
	}
	defer us.Close()

	vanePort, err := setupVaneForWS(s, true, us.Port)
	if err != nil {
		return err
	}

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := performUpgradeHandshake(target, true)
	if err != nil {
		return term.FormatFailure("Handshake failed", term.NewNode(err.Error()))
	}
	defer conn.Close()

	// Test Echo
	msg := "Hello WebSocket"
	conn.Write([]byte(msg))

	buf := make([]byte, len(msg))
	_, err = io.ReadFull(conn, buf)
	if err != nil {
		return term.FormatFailure("Read failed", term.NewNode(err.Error()))
	}

	if string(buf) != msg {
		return term.FormatFailure("Echo mismatch", term.NewNode(fmt.Sprintf("Got: %s", string(buf))))
	}

	return nil
}

// Test 3: Large Stream (1GB)
func TestWSStreamLarge(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	const PayloadSize = 1024 * 1024 * 1024 // 1GB

	if debug {
		term.Info("Preparing WebSocket 1GB Streaming Test")
	}

	us, err := mock.NewWsUpstream()
	if err != nil {
		return err
	}
	defer us.Close()

	vanePort, err := setupVaneForWS(s, true, us.Port)
	if err != nil {
		return err
	}

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	target := fmt.Sprintf("127.0.0.1:%d", vanePort)
	conn, err := performUpgradeHandshake(target, true)
	if err != nil {
		return term.FormatFailure("Handshake failed", term.NewNode(err.Error()))
	}
	defer conn.Close()

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
		if _, err := io.Copy(conn, reader); err != nil {
			errChan <- fmt.Errorf("Write Error: %w", err)
		}
		// In a real WS, we would send a Close frame. Here we can close write side if possible,
		// but since it's TLS, usually we just close conn.
		// For this test, we rely on byte count.
	}()

	// Reader
	go func() {
		defer wg.Done()
		hasher := sha256.New()
		// Limit read to PayloadSize to verify exact transmission
		if _, err := io.CopyN(hasher, conn, PayloadSize); err != nil {
			errChan <- fmt.Errorf("Read Error: %w", err)
			return
		}
		actualHash := hex.EncodeToString(hasher.Sum(nil))
		if actualHash != expectedHash {
			errChan <- fmt.Errorf("Hash Mismatch")
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
		// Success implies reader finished successfully
	case err := <-errChan:
		return term.FormatFailure("Streaming Failed", term.NewNode(err.Error()))
	case <-time.After(60 * time.Second):
		return term.FormatFailure("Timeout", nil)
	}

	if debug {
		term.Pass("1GB WebSocket Tunnel Verified")
	}

	return nil
}
