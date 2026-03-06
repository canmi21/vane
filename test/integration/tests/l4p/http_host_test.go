/* integration/tests/l4p/http_host_test.go */
package l4p

import (
	"bufio"
	"encoding/json"
	"fmt"
	"net"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/mock"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestHttpHostProxy(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	srv, _ := mock.NewTcpEchoServer()
	defer srv.Close()

	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l4Flow := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("http"),
	}
	l4Bytes, _ := json.Marshal(l4Flow)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// L4+: IF {{http.host}} == "vane.local" THEN Proxy ELSE Abort
	l4pFlow := advanced.L4FlowConfig{
		Connection: advanced.NewMatch(
			"{{http.host}}",
			"vane.local",
			advanced.NewTransparentProxy("127.0.0.1", srv.Port),
			advanced.NewAbortConnection(),
		),
	}
	l4pBytes, _ := json.Marshal(l4pFlow)
	sb.WriteConfig("resolver/http.json", l4pBytes)

	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// Wait for port to be ready
	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", vanePort), 1*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()

	// Send HTTP Request
	fmt.Fprintf(conn, "GET / HTTP/1.1\r\nHost: vane.local\r\n\r\n")

	// Expect Echo Back
	line, err := bufio.NewReader(conn).ReadString('\n')

	// FIXED: The Mock TCP Server uses bufio.Scanner which strips CR/LF,
	// and then appends only \n when echoing.
	// So we expect "GET / HTTP/1.1\n", NOT "\r\n".
	expected := "GET / HTTP/1.1\n"

	if err != nil || line != expected {
		t.Fatal(term.FormatFailure("HTTP Host Routing Failed",
			term.NewNode(fmt.Sprintf("Want: %q, Got: %q", expected, line))))
	}
}
