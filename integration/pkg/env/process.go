/* integration/pkg/env/process.go */
package env

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"
)

// Key type for context to avoid collisions (SA1029)
type ContextKey string

const DebugKey ContextKey = "debug"

type Process struct {
	cmd       *exec.Cmd
	sandbox   *Sandbox
	LogBuffer *bytes.Buffer
	DebugMode bool
}

// generateAccessToken creates a random 32-character hex token for testing
func generateAccessToken() string {
	bytes := make([]byte, 16)
	if _, err := rand.Read(bytes); err != nil {
		// Fallback to a static token if random generation fails
		return "test-token-fallback-1234567890abcdef"
	}
	return hex.EncodeToString(bytes)
}

func (s *Sandbox) StartVane(ctx context.Context, debugMode bool) (*Process, error) {
	return s.startVaneInternal(ctx, debugMode, true)
}

// StartVaneWithoutToken starts Vane without ACCESS_TOKEN (for testing no-console mode)
func (s *Sandbox) StartVaneWithoutToken(ctx context.Context, debugMode bool) (*Process, error) {
	return s.startVaneInternal(ctx, debugMode, false)
}

func (s *Sandbox) startVaneInternal(ctx context.Context, debugMode bool, withToken bool) (*Process, error) {
	cmd := exec.CommandContext(ctx, "vane")

	logLevel := "info"
	if debugMode {
		logLevel = "debug"
	}

	envVars := []string{
		fmt.Sprintf("CONFIG_DIR=%s", s.ConfigDir),
		fmt.Sprintf("SOCKET_DIR=%s", s.SocketDir),
		fmt.Sprintf("PORT=%d", s.ConsolePort),
		fmt.Sprintf("LOG_LEVEL=%s", logLevel),
		"DETECT_PUBLIC_NETWORK=false",
		"CONSOLE_LISTEN_IPV6=false",
		"DEV_PROJECT_DIR=/tmp/void",
	}

	// Add ACCESS_TOKEN for default tests (enables management console)
	if withToken {
		token := generateAccessToken()
		envVars = append(envVars, fmt.Sprintf("ACCESS_TOKEN=%s", token))
	}

	cmd.Env = append(os.Environ(), envVars...)

	// FIXED: Always initialize buffer to allow WaitForLog to work
	logBuf := &bytes.Buffer{}

	if debugMode {
		// In debug mode, write to BOTH stdout/stderr AND the buffer
		cmd.Stdout = io.MultiWriter(os.Stdout, logBuf)
		cmd.Stderr = io.MultiWriter(os.Stderr, logBuf)
	} else {
		cmd.Stdout = logBuf
		cmd.Stderr = logBuf
	}

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("failed to start vane binary: %w", err)
	}

	proc := &Process{
		cmd:       cmd,
		sandbox:   s,
		LogBuffer: logBuf,
		DebugMode: debugMode,
	}

	// Wait strategy depends on whether ACCESS_TOKEN is set
	if withToken {
		// With token: wait for console port to be ready
		if err := proc.WaitForReady(5 * time.Second); err != nil {
			proc.Stop()
			if !debugMode {
				return nil, fmt.Errorf("vane startup failed: %w\nLogs:\n%s", err, logBuf.String())
			}
			return nil, fmt.Errorf("vane startup failed: %w", err)
		}
		// Console is ready. Business ports will be initialized in background.
		// Tests should wait for specific port logs like "PORT XX TCP UP" if needed.
	} else {
		// Without token: console port won't start, just wait for startup logs
		if err := proc.WaitForLog("ACCESS_TOKEN not set", 3*time.Second); err != nil {
			proc.Stop()
			if !debugMode {
				return nil, fmt.Errorf("vane startup failed: %w\nLogs:\n%s", err, logBuf.String())
			}
			return nil, fmt.Errorf("vane startup failed: %w", err)
		}
		// Tests should wait for specific port logs like "PORT XX TCP UP" if needed.
	}

	return proc, nil
}

func (p *Process) WaitForReady(timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	target := fmt.Sprintf("127.0.0.1:%d", p.sandbox.ConsolePort)

	for {
		select {
		case <-ticker.C:
			conn, err := net.DialTimeout("tcp", target, 50*time.Millisecond)
			if err == nil {
				conn.Close()
				return nil
			}
			if p.cmd.ProcessState != nil && p.cmd.ProcessState.Exited() {
				return fmt.Errorf("process exited unexpectedly")
			}
		case <-time.After(time.Until(deadline)):
			return fmt.Errorf("timeout waiting for port %d", p.sandbox.ConsolePort)
		}
	}
}

// WaitForNoConsole verifies that the console port is NOT listening (for no-token mode)
func (p *Process) WaitForNoConsole(timeout time.Duration) error {
	target := fmt.Sprintf("127.0.0.1:%d", p.sandbox.ConsolePort)

	// Wait for the log message to ensure Vane has started
	if err := p.WaitForLog("ACCESS_TOKEN not set", timeout); err != nil {
		return fmt.Errorf("expected 'ACCESS_TOKEN not set' log message: %w", err)
	}

	// Verify console port is NOT listening
	for i := 0; i < 5; i++ {
		conn, err := net.DialTimeout("tcp", target, 50*time.Millisecond)
		if err == nil {
			conn.Close()
			return fmt.Errorf("console port %d should NOT be listening (no ACCESS_TOKEN)", p.sandbox.ConsolePort)
		}

		if p.cmd.ProcessState != nil && p.cmd.ProcessState.Exited() {
			return fmt.Errorf("process exited unexpectedly")
		}

		time.Sleep(200 * time.Millisecond)
	}

	return nil
}

// WaitForLog polls the log buffer until a substring appears or timeout occurs.
func (p *Process) WaitForLog(snippet string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			logs := p.LogBuffer.String()
			if strings.Contains(logs, snippet) {
				return nil
			}
			if p.cmd.ProcessState != nil && p.cmd.ProcessState.Exited() {
				return fmt.Errorf("process exited while waiting for log: %s", snippet)
			}
		case <-time.After(time.Until(deadline)):
			return fmt.Errorf("timeout waiting for log snippet: '%s'", snippet)
		}
	}
}

// WaitForTcpPort waits for a TCP port to be ready (looks for "PORT {port} TCP UP" in logs)
func (p *Process) WaitForTcpPort(port int, timeout time.Duration) error {
	return p.WaitForLog(fmt.Sprintf("PORT %d TCP UP", port), timeout)
}

// WaitForUdpPort waits for a UDP port to be ready (looks for "PORT {port} UDP UP" in logs)
func (p *Process) WaitForUdpPort(port int, timeout time.Duration) error {
	return p.WaitForLog(fmt.Sprintf("PORT %d UDP UP", port), timeout)
}

func (p *Process) Stop() error {
	if p.cmd.Process == nil {
		return nil
	}
	if err := p.cmd.Process.Signal(os.Interrupt); err != nil {
		return p.cmd.Process.Kill()
	}
	return p.cmd.Wait()
}

func (p *Process) DumpLogs() string {
	if p.LogBuffer != nil {
		return p.LogBuffer.String()
	}
	return "(No logs captured)"
}
