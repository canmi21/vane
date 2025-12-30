/* integration/tests/l4/test_tcp_flow_ratelimit.go */
package l4

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"sync"
	"sync/atomic"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestTcpFlowRateLimit(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Upstream Echo Server
	srv, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	limitPerSec := 20

	// Configuration:
	// RateLimit(IP, 20/s) ? Proxy(Srv) : Abort()
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewRateLimitSec(
			"{{conn.ip}}", // Rate limit by Source IP
			limitPerSec,
			advanced.NewTransparentProxy("127.0.0.1", srv.Port), // Pass
			advanced.NewAbortConnection(),                       // Block
		),
	}

	jsonBytes, _ := json.Marshal(flowConf)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), jsonBytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// --- Phase 1: Burst Test ---
	// Send 100 requests concurrently.
	// Expected passed: [20, 40]
	// Why up to 40? Because the test might span across the 1-second boundary of the limiter's interval.

	totalRequests := 100
	var successCount int64
	var wg sync.WaitGroup

	// Use a semaphore to limit local client concurrency slightly to avoid OS socket exhaustion issues,
	// though 100 is usually fine.
	sem := make(chan struct{}, 50)

	for i := 0; i < totalRequests; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			sem <- struct{}{}
			defer func() { <-sem }()

			if checkConnection(vanePort) {
				atomic.AddInt64(&successCount, 1)
			}
		}()
	}

	wg.Wait()
	// Assertion logic
	// Lower bound: strictly the limit (20)
	// Upper bound: 2 * limit (40) to account for boundary crossing
	if successCount < int64(limitPerSec) {
		return term.FormatFailure(
			fmt.Sprintf("Rate limiter blocked too aggressively. Got %d, want >= %d", successCount, limitPerSec),
			nil,
		)
	}
	if successCount > int64(limitPerSec*2) {
		return term.FormatFailure(
			fmt.Sprintf("Rate limiter failed to block enough. Got %d, want <= %d", successCount, limitPerSec*2),
			nil,
		)
	}

	// --- Phase 2: Recovery Test ---
	// Wait > 1 second for the bucket/interval to reset.
	time.Sleep(1200 * time.Millisecond)

	// Send exactly limitPerSec requests. All should pass.
	atomic.StoreInt64(&successCount, 0)
	for i := 0; i < limitPerSec; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if checkConnection(vanePort) {
				atomic.AddInt64(&successCount, 1)
			}
		}()
	}
	wg.Wait()

	if successCount != int64(limitPerSec) {
		return term.FormatFailure(
			fmt.Sprintf("Rate limiter did not reset properly. Got %d, want %d", successCount, limitPerSec),
			nil,
		)
	}

	return nil
}

// checkConnection tries to connect and send data.
// Returns true if echoed back successfully, false if connection closed/aborted.
func checkConnection(port int) bool {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), 200*time.Millisecond)
	if err != nil {
		return false
	}
	defer conn.Close()

	// Set strict deadlines
	conn.SetDeadline(time.Now().Add(500 * time.Millisecond))

	payload := "ping\n"
	if _, err := fmt.Fprintf(conn, "%s", payload); err != nil {
		return false
	}

	// Try to read 1 byte.
	// If aborted, Read returns EOF or error immediately.
	// If proxying, we get data back.
	oneByte := make([]byte, 1)
	_, err = conn.Read(oneByte)
	return err == nil
}
