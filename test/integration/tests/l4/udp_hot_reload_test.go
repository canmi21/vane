/* test/integration/tests/l4/udp_hot_reload_test.go */

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

func TestUdpHotReload(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	resp1 := []byte("Response-1")
	resp2 := []byte("Response-2")

	srv1, _ := mock.NewUdpFixedResponseServer(resp1)
	defer srv1.Close()
	srv2, _ := mock.NewUdpFixedResponseServer(resp2)
	defer srv2.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]
	configFile := fmt.Sprintf("listener/[%d]/udp.yaml", vanePort)

	writeConf := func(targetPort int) error {
		conf := basic.LegacyUdpConfig{
			Protocols: []basic.UdpProtocolRule{
				{
					// FIXED: Name must be strictly [a-z0-9]+
					Name:     "udpreload",
					Priority: 1,
					Detect:   basic.Detect{Method: basic.DetectFallback, Pattern: "any"},
					Destination: basic.UdpDestination{
						Type: "forward",
						Forward: &basic.Forward{
							Strategy: basic.StrategyRandom,
							Targets:  []basic.Target{{Ip: "127.0.0.1", Port: targetPort}},
						},
					},
				},
			},
		}
		bytes, _ := yaml.Marshal(conf)
		return sb.WriteConfig(configFile, bytes)
	}

	// 1. Initial: Target Srv1
	if err := writeConf(srv1.Port); err != nil {
		t.Fatal(err)
	}

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForUdpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(err)
	}

	if val, err := sendUdp(vanePort); err != nil || val != string(resp1) {
		t.Fatal(term.FormatFailure("Initial routing failed", term.NewNode(fmt.Sprintf("Got %s want %s", val, resp1))))
	}

	// 2. Reload: Target Srv2
	if err := writeConf(srv2.Port); err != nil {
		t.Fatal(err)
	}

	if err := proc.WaitForLog("UDP RELOAD", 3*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Reload log missing", term.NewNode(err.Error())))
	}
	time.Sleep(100 * time.Millisecond)

	if val, err := sendUdp(vanePort); err != nil || val != string(resp2) {
		root := term.NewNode("Reload Routing Failed")
		root.Add(fmt.Sprintf("Expected: %s", resp2))
		root.Add(fmt.Sprintf("Actual:   %s", val))
		if err != nil {
			root.Add(fmt.Sprintf("Error: %v", err))
		}
		t.Fatal(term.FormatFailure("Hot swap check", root))
	}
}

func sendUdp(port int) (string, error) {
	conn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return "", err
	}
	defer conn.Close()
	conn.SetDeadline(time.Now().Add(1 * time.Second))

	if _, err := conn.Write([]byte("ping")); err != nil {
		return "", err
	}
	buf := make([]byte, 1024)
	n, err := conn.Read(buf)
	if err != nil {
		return "", err
	}
	return string(buf[:n]), nil
}
