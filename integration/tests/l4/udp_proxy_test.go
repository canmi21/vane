/* integration/tests/l4/udp_proxy_test.go */
package l4

import (
	"bytes"
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

func TestUdpProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// DNS-like magic bytes to trigger detection
	// \x00\x01 is the pattern
	magicPacket := []byte{0x00, 0x01, 0xAA, 0xBB}
	expectedResponse := []byte{0x00, 0x01, 0xCA, 0xFE}

	// 1. Setup Upstream Mock
	upstream, err := mock.NewUdpFixedResponseServer(expectedResponse)
	if err != nil {
		t.Fatal(err)
	}
	defer upstream.Close()

	// 2. Setup Vane Config
	ports, err := env.GetFreePorts(1)
	if err != nil {
		t.Fatal(err)
	}
	vanePort := ports[0]

	udpConf := basic.LegacyUdpConfig{
		Protocols: []basic.UdpProtocolRule{
			{
				Name:     "magicudp",
				Priority: 50,
				Detect: basic.Detect{
					Method: basic.DetectPrefix,
					// FIXED: Use actual Go byte string. yaml.v3 will handle quoting correctly for Rust.
					Pattern: "\x00\x01",
				},
				Destination: basic.UdpDestination{
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

	confBytes, err := yaml.Marshal(udpConf)
	if err != nil {
		t.Fatal(err)
	}
	if err := sb.WriteConfig(fmt.Sprintf("listener/[%d]/udp.yaml", vanePort), confBytes); err != nil {
		t.Fatal(err)
	}

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// FIXED: Wait for Listener UP *BEFORE* sending packets.
	// UDP is connectionless; if we send before Vane is ready, the packet is lost.
	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error())))
	}

	// 4. Send Packet
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", vanePort))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	if _, err := conn.Write(magicPacket); err != nil {
		t.Fatal(err)
	}

	conn.SetReadDeadline(time.Now().Add(2 * time.Second))
	buf := make([]byte, 1024)
	n, err := conn.Read(buf)
	if err != nil {
		root := term.NewNode("UDP Read Error")
		root.Add(err.Error())
		if !debug {
			root.Add("Logs").Add(proc.DumpLogs())
		}
		t.Fatal(term.FormatFailure("No response from Vane UDP", root))
	}

	recv := buf[:n]
	if !bytes.Equal(recv, expectedResponse) {
		root := term.NewNode("Payload Mismatch")
		root.Add(fmt.Sprintf("Expected: %x", expectedResponse))
		root.Add(fmt.Sprintf("Actual:   %x", recv))
		t.Fatal(term.FormatFailure("Wrong UDP Response", root))
	}
}
