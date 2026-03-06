/* integration/tests/l4/test_udp_proxy.go */
package l4

import (
	"bytes"
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

func TestUdpProxy(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// DNS-like magic bytes to trigger detection
	// \x00\x01 is the pattern
	magicPacket := []byte{0x00, 0x01, 0xAA, 0xBB}
	expectedResponse := []byte{0x00, 0x01, 0xCA, 0xFE}

	// 1. Setup Upstream Mock
	upstream, err := mock.NewUdpFixedResponseServer(expectedResponse)
	if err != nil {
		return err
	}
	defer upstream.Close()

	// 2. Setup Vane Config
	ports, err := env.GetFreePorts(1)
	if err != nil {
		return err
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
		return err
	}
	if err := s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.yaml", vanePort), confBytes); err != nil {
		return err
	}

	// 3. Start Vane
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// FIXED: Wait for Listener UP *BEFORE* sending packets.
	// UDP is connectionless; if we send before Vane is ready, the packet is lost.
	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error()))
	}

	// 4. Send Packet
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", vanePort))
	if err != nil {
		return err
	}
	defer conn.Close()

	if _, err := conn.Write(magicPacket); err != nil {
		return err
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
		return term.FormatFailure("No response from Vane UDP", root)
	}

	recv := buf[:n]
	if !bytes.Equal(recv, expectedResponse) {
		root := term.NewNode("Payload Mismatch")
		root.Add(fmt.Sprintf("Expected: %x", expectedResponse))
		root.Add(fmt.Sprintf("Actual:   %x", recv))
		return term.FormatFailure("Wrong UDP Response", root)
	}

	return nil
}
