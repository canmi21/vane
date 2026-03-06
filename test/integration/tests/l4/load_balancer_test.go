/* test/integration/tests/l4/load_balancer_test.go */

package l4

import (
	"fmt"
	"net"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

// TrackingTcpServer counts incoming connections.
type TrackingTcpServer struct {
	Listener net.Listener
	Port     int
	conns    int64
	wg       sync.WaitGroup
}

func NewTrackingServer() (*TrackingTcpServer, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	s := &TrackingTcpServer{
		Listener: l,
		Port:     l.Addr().(*net.TCPAddr).Port,
	}
	s.wg.Add(1)
	go s.serve()
	return s, nil
}

func (s *TrackingTcpServer) serve() {
	defer s.wg.Done()
	for {
		conn, err := s.Listener.Accept()
		if err != nil {
			return
		}
		atomic.AddInt64(&s.conns, 1)
		conn.Close() // Close immediately, we just count
	}
}

func (s *TrackingTcpServer) Close() {
	s.Listener.Close()
	s.wg.Wait()
}

func (s *TrackingTcpServer) Count() int64 {
	return atomic.LoadInt64(&s.conns)
}

func startTrackingBackends(count int) ([]*TrackingTcpServer, error) {
	var servers []*TrackingTcpServer
	for i := 0; i < count; i++ {
		s, err := NewTrackingServer()
		if err != nil {
			for _, existing := range servers {
				existing.Close()
			}
			return nil, err
		}
		servers = append(servers, s)
	}
	return servers, nil
}

func TestLoadBalancerRandom(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup 3 Healthy Backends
	healthyServers, err := startTrackingBackends(3)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		for _, s := range healthyServers {
			s.Close()
		}
	}()

	// 2. Setup 2 Unhealthy Targets (just reserve ports)
	unhealthyPorts, err := env.GetFreePorts(2)
	if err != nil {
		t.Fatal(err)
	}

	// 3. Configure Vane
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	targets := []basic.Target{}
	for _, s := range healthyServers {
		targets = append(targets, basic.Target{Ip: "127.0.0.1", Port: s.Port})
	}
	for _, p := range unhealthyPorts {
		targets = append(targets, basic.Target{Ip: "127.0.0.1", Port: p})
	}

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "lb_random",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets:  targets,
					},
				},
			},
		},
	}

	confBytes, _ := yaml.Marshal(config)
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), confBytes); err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// --- Phase 1: Priming / Stabilization ---
	stabilized := false
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
		if err == nil {
			conn.Write([]byte("ping\n"))
			conn.Close()
		}
		time.Sleep(50 * time.Millisecond)

		activeCount := 0
		for _, s := range healthyServers {
			if s.Count() > 0 {
				activeCount++
			}
		}
		if activeCount == 3 {
			stabilized = true
			break
		}
	}

	if !stabilized {
		t.Fatal(term.FormatFailure("Failed to stabilize load balancer pool", term.NewNode(fmt.Sprintf("Only subset active.\nLogs:\n%s", proc.DumpLogs()))))
	}

	// Reset counters (logically)
	baselineCounts := make([]int64, len(healthyServers))
	for i, s := range healthyServers {
		baselineCounts[i] = s.Count()
	}

	// 4. Run Load Test (300 requests)
	requestCount := 300
	for i := 0; i < requestCount; i++ {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
		if err != nil {
			t.Fatal(term.FormatFailure("Failed to connect to LB", term.NewNode(err.Error())))
		}
		conn.Write([]byte("ping\n"))
		conn.Close()
	}

	// 5. Verify Distribution
	var total int64
	for i, srv := range healthyServers {
		current := srv.Count()
		c := current - baselineCounts[i] // Use increment
		total += c
		if c < 50 || c > 150 {
			t.Fatal(term.FormatFailure(fmt.Sprintf("Server %d load unbalanced", i), term.NewNode(fmt.Sprintf("Count: %d (Expected ~100)", c))))
		}
	}

	// Note: total may differ slightly from requestCount due to connection close races.
	// Random strategy only picks from 'available_targets', so small deltas are acceptable.
	_ = total
}

func TestLoadBalancerSerial(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	healthyServers, err := startTrackingBackends(3)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		for _, s := range healthyServers {
			s.Close()
		}
	}()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	targets := []basic.Target{}
	for _, s := range healthyServers {
		targets = append(targets, basic.Target{Ip: "127.0.0.1", Port: s.Port})
	}

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "lb_serial",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategySerial,
						Targets:  targets,
					},
				},
			},
		},
	}

	confBytes, _ := yaml.Marshal(config)
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), confBytes); err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// --- Phase 1: Priming / Stabilization ---
	// Wait until Vane sees all 3 targets as healthy.
	// We do this by sending requests until we verify that ALL backends have received traffic.
	// This confirms the load balancer pool has stabilized to [A, B, C].
	stabilized := false
	deadline := time.Now().Add(10 * time.Second)

	for time.Now().Before(deadline) {
		// Send a probe
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
		if err == nil {
			conn.Write([]byte("ping\n"))
			conn.Close()
		}
		time.Sleep(50 * time.Millisecond)

		// Check if all servers have seen traffic
		activeCount := 0
		for _, s := range healthyServers {
			if s.Count() > 0 {
				activeCount++
			}
		}

		if activeCount == 3 {
			stabilized = true
			break
		}
	}

	if !stabilized {
		t.Fatal(term.FormatFailure("Failed to stabilize load balancer pool", term.NewNode(fmt.Sprintf("Only subset of backends active after 10s.\nLogs:\n%s", proc.DumpLogs()))))
	}

	// Reset counters (logically) by taking a snapshot
	baselineCounts := make([]int64, len(healthyServers))
	for i, s := range healthyServers {
		baselineCounts[i] = s.Count()
	}

	// --- Phase 2: Strict Distribution Test ---
	// Send 30 requests. Since pool is stable (size 3), Round-Robin MUST distribute exactly 10 to each.
	requestCount := 30
	for i := 0; i < requestCount; i++ {
		conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 500*time.Millisecond)
		if err != nil {
			t.Fatal(term.FormatFailure("Failed to connect", term.NewNode(err.Error())))
		}
		conn.Write([]byte("ping\n"))
		conn.Close()
		time.Sleep(10 * time.Millisecond)
	}

	// Verify Exact Distribution (Increment should be 10)
	for i, srv := range healthyServers {
		current := srv.Count()
		increment := current - baselineCounts[i]
		if increment != 10 {
			t.Fatal(term.FormatFailure(fmt.Sprintf("Server %d count mismatch (Serial)", i), term.NewNode(fmt.Sprintf("Got +%d, Expected +10 (Total: %d)", increment, current))))
		}
	}
}
