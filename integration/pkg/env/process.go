/* integration/pkg/env/process.go */
package env

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
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

func (s *Sandbox) StartVane(ctx context.Context, debugMode bool) (*Process, error) {
	cmd := exec.CommandContext(ctx, "vane")

	logLevel := "info"
	if debugMode {
		logLevel = "debug"
	}

	cmd.Env = append(os.Environ(),
		fmt.Sprintf("CONFIG_DIR=%s", s.ConfigDir),
		fmt.Sprintf("SOCKET_DIR=%s", s.SocketDir),
		fmt.Sprintf("PORT=%d", s.ConsolePort),
		fmt.Sprintf("LOG_LEVEL=%s", logLevel),
		"DETECT_PUBLIC_NETWORK=false",
		"CONSOLE_LISTEN_IPV6=false",
		"DEV_PROJECT_DIR=/tmp/void",
	)

	var logBuf *bytes.Buffer
	if debugMode {
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
	} else {
		logBuf = &bytes.Buffer{}
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

	if err := proc.WaitForReady(5 * time.Second); err != nil {
		proc.Stop()
		if !debugMode && logBuf != nil {
			// Compact error format
			return nil, fmt.Errorf("vane startup failed: %w\nLogs:\n%s", err, logBuf.String())
		}
		return nil, fmt.Errorf("vane startup failed: %w", err)
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
	return "(Logs streamed)"
}
