/* integration/tests/l4/target_resolution_test.go */
package l4

import (
	"fmt"
	"net"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/basic"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
	"gopkg.in/yaml.v3"
)

func TestResolveIp(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	backend, _ := mock.NewTcpEchoServer()
	defer backend.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "resolve_ip",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Ip: "127.0.0.1", Port: backend.Port},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(config)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(err)
	}

	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to connect via IP", term.NewNode(err.Error())))
	}
	conn.Close()
}

func TestResolveNode(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	backend, _ := mock.NewTcpEchoServer()
	defer backend.Close()

	// 1. Write nodes.yml
	nodesConfig := map[string]interface{}{
		"nodes": []map[string]interface{}{
			{
				"name": "target-node",
				"ips": []map[string]interface{}{
					{
						"address": "127.0.0.1",
						"ports":   []int{backend.Port},
						"type":    "ipv4",
					},
				},
			},
		},
	}
	nodeBytes, _ := yaml.Marshal(nodesConfig)
	sb.WriteConfig("nodes.yml", nodeBytes)

	// 2. Configure Vane with Node Target
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "resolve_node",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Node: "target-node", Port: backend.Port},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(config)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(err)
	}

	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to connect via Node", term.NewNode(err.Error())))
	}
	conn.Close()
}

func TestResolveDomain(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	backend, _ := mock.NewTcpEchoServer()
	defer backend.Close()

	// 1. Setup Mock DNS Server (Minimal UDP)
	dnsAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	dnsConn, err := net.ListenUDP("udp", dnsAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer dnsConn.Close()
	dnsPort := dnsConn.LocalAddr().(*net.UDPAddr).Port

	// DNS Resolver Task: Always return 127.0.0.1 for any A query
	go func() {
		buf := make([]byte, 512)
		for {
			n, remote, err := dnsConn.ReadFromUDP(buf)
			if err != nil {
				return
			}
			if n < 12 {
				continue
			}
			// Minimal DNS response for A record (127.0.0.1)
			resp := make([]byte, n+16)
			copy(resp, buf[:n])
			resp[2] = 0x81
			resp[3] = 0x80 // Response, No Error
			resp[7] = 1    // 1 Answer
			ans := []byte{0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 127, 0, 0, 1}
			copy(resp[n:], ans)
			dnsConn.WriteToUDP(resp[:n+16], remote)
		}
	}()

	// 2. Configure Vane with Domain Target
	sb.Env["NAMESERVER1"] = "127.0.0.1"
	sb.Env["NAMESERVER1_PORT"] = fmt.Sprintf("%d", dnsPort)

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "resolve_domain",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Domain: "vane.local", Port: backend.Port},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(config)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(err)
	}

	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(term.FormatFailure("Failed to connect via Domain", term.NewNode(err.Error())))
	}
	conn.Close()
}

func TestResolveDomainFailure(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Mock DNS Server (Returns NXDomain)
	dnsAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	dnsConn, err := net.ListenUDP("udp", dnsAddr)
	if err != nil {
		t.Fatal(err)
	}
	defer dnsConn.Close()
	dnsPort := dnsConn.LocalAddr().(*net.UDPAddr).Port

	// DNS Resolver Task: Always return NXDomain
	go func() {
		buf := make([]byte, 512)
		for {
			n, remote, err := dnsConn.ReadFromUDP(buf)
			if err != nil {
				return
			}
			if n < 12 {
				continue
			}
			// Construct NXDomain Response
			resp := make([]byte, n)
			copy(resp, buf[:n])
			resp[2] = 0x81 // Response
			resp[3] = 0x83 // Recursion Available + RCODE=3 (NXDomain)
			resp[7] = 0    // 0 Answers

			dnsConn.WriteToUDP(resp, remote)
		}
	}()

	// 2. Configure Vane with Mock DNS
	sb.Env["NAMESERVER1"] = "127.0.0.1"
	sb.Env["NAMESERVER1_PORT"] = fmt.Sprintf("%d", dnsPort)

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	config := basic.LegacyTcpConfig{
		Protocols: []basic.TcpProtocolRule{
			{
				Name:     "resolve_domain_fail",
				Priority: 1,
				Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
				Destination: basic.TcpDestination{
					Type: "forward",
					Forward: &basic.Forward{
						Strategy: basic.StrategyRandom,
						Targets: []basic.Target{
							{Domain: "unresolved.vane", Port: 8080},
						},
					},
				},
			},
		},
	}

	bytes, _ := yaml.Marshal(config)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.yaml", vanePort), bytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(err)
	}

	// Traffic should fail as no targets available
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err == nil {
		buf := make([]byte, 10)
		conn.SetReadDeadline(time.Now().Add(500 * time.Millisecond))
		_, err := conn.Read(buf)
		conn.Close()
		if err == nil {
			t.Fatal(term.FormatFailure("Traffic should have been blocked (DNS Fail)", nil))
		}
	}
}
