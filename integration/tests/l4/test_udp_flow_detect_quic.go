/* integration/tests/l4/test_udp_flow_detect_quic.go */
package l4

import (
	"context"
	"encoding/json"
	"fmt"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestUdpFlowDetectQuic(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	expectedResponse := []byte("QUIC-ACK")
	srv, _ := mock.NewUdpFixedResponseServer(expectedResponse)
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// Detect(QUIC) ? Proxy(Srv) : Abort()
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewProtocolDetect(
			"quic",
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}

	jsonBytes, _ := json.Marshal(flowConf)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/udp.json", vanePort), jsonBytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		return term.FormatFailure("UDP Listener failed to start", term.NewNode(err.Error()))
	}

	// 4. Positive Test: Valid QUIC Initial Packet
	const (
		HeaderFormLong    = 0x80
		FixedBit          = 0x40
		PacketTypeInitial = 0x00
		Version1          = 0x01
	)
	packet := make([]byte, 1200)
	packet[0] = HeaderFormLong | FixedBit | PacketTypeInitial
	packet[4] = Version1

	if err := verifyUdpResponse(vanePort, packet, true); err != nil {
		return term.FormatFailure("Positive Check Failed (QUIC)", term.NewNode(err.Error()))
	}

	// 5. Negative Test: Garbage
	garbage := []byte("NOT_QUIC_PACKET")
	if err := verifyUdpResponse(vanePort, garbage, false); err != nil {
		return term.FormatFailure("Negative Check Failed (Garbage)", term.NewNode(err.Error()))
	}

	return nil
}
