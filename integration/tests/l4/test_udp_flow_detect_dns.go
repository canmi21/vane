/* integration/tests/l4/test_udp_flow_detect_dns.go */
package l4

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestUdpFlowDetectDns(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	expectedResponse := []byte("DNS-ACK")
	srv, _ := mock.NewUdpFixedResponseServer(expectedResponse)
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	// Detect(DNS) ? Proxy(Srv) : Abort()
	flowConf := advanced.L4FlowConfig{
		Connection: advanced.NewProtocolDetect(
			"dns",
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

	// 4. Positive Test: Valid DNS Query
	dnsPacket := []byte{
		0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
		0x03, 'w', 'w', 'w', 0x00, 0x00, 0x01, 0x00, 0x01,
	}
	if err := verifyUdpResponse(vanePort, dnsPacket, true); err != nil {
		return term.FormatFailure("Positive Check Failed (DNS)", term.NewNode(err.Error()))
	}

	// 5. Negative Test: Garbage (Too short or wrong flags)
	garbage := []byte("NOT_DNS")
	if err := verifyUdpResponse(vanePort, garbage, false); err != nil {
		return term.FormatFailure("Negative Check Failed (Garbage)", term.NewNode(err.Error()))
	}

	return nil
}

func verifyUdpResponse(port int, payload []byte, expectSuccess bool) error {
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return err
	}
	defer conn.Close()

	conn.SetDeadline(time.Now().Add(1 * time.Second))
	if _, err := conn.Write(payload); err != nil {
		return err
	}

	buf := make([]byte, 1024)
	n, err := conn.Read(buf)

	if expectSuccess {
		if err != nil {
			return fmt.Errorf("expected response but got error: %v", err)
		}
		if n == 0 {
			return fmt.Errorf("got empty response")
		}
	} else {
		// Expect Timeout (no response)
		if err == nil {
			return fmt.Errorf("expected timeout/no-response but got data: %x", buf[:n])
		}
		// Check if error is indeed a timeout
		if netErr, ok := err.(net.Error); !ok || !netErr.Timeout() {
			return fmt.Errorf("expected timeout but got other error: %v", err)
		}
	}
	return nil
}
