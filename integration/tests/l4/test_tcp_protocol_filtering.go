/* integration/tests/l4/test_tcp_protocol_filtering.go */
package l4

import (
	"context"
	"fmt"
	"net"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

func TestTcpProtocolFiltering(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock Backend
	upstream, err := mock.NewTcpEchoServer()
	if err != nil {
		return err
	}
	defer upstream.Close()

	// 2. Setup Vane Config (Legacy)
	// Rule: only forward HTTP-like traffic, drop everything else (implicitly)
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
	}
	vanePort := ports[0]

	tcpConf := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "httpfilter",
				Priority: 1,
				Detect: basic.Detect{
					Method:  basic.DetectRegex,
					Pattern: "^[A-Z]+ /.* HTTP/1\\.[01]",
				},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Ip: "127.0.0.1", Port: upstream.Port},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(tcpConf)
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes); err != nil {
		return err
	}

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("Port failed to start", term.NewNode(err.Error()))
	}

	// 4. Test Scenario 1: Valid HTTP Traffic (Match)
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		return term.FormatFailure("Failed to connect for HTTP test", nil)
	}
	fmt.Fprintf(conn, "GET / HTTP/1.1\r\n\r\n")
	buf := make([]byte, 1024)
	n, err := conn.Read(buf)
	conn.Close()

	if err != nil || n == 0 {
		return term.FormatFailure("Valid HTTP was filtered out", nil)
	}

	// 5. Test Scenario 2: TLS Traffic (Mismatch)
	// Vane should see this doesn't match the regex, and since there's no fallback,
	// it should eventually close the connection or stall.
	tlsHandshake := []byte{0x16, 0x03, 0x01, 0x00, 0x55}
	conn2, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		return term.FormatFailure("Failed to connect for TLS test", nil)
	}
	conn2.Write(tlsHandshake)

	// Wait a bit. Vane should close the connection after detection failure (no rules match).
	// We'll try to read. Expect EOF or error.
	conn2.SetReadDeadline(time.Now().Add(1 * time.Second))
	_, err = conn2.Read(buf)
	conn2.Close()

	if err == nil {
		return term.FormatFailure("Mismatching traffic (TLS) was NOT filtered out", nil)
	}

	return nil
}
