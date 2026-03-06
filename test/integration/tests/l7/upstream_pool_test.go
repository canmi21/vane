/* test/integration/tests/l7/upstream_pool_test.go */

package l7

import (
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"sync/atomic"
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

type trackingListener struct {
	net.Listener
	accepts *int64
}

func (l *trackingListener) Accept() (net.Conn, error) {
	conn, err := l.Listener.Accept()
	if err == nil {
		atomic.AddInt64(l.accepts, 1)
	}
	return conn, err
}

func TestUpstreamConnectionPooling(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Setup Counting Backend
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var accepts int64
	trackedL := &trackingListener{Listener: l, accepts: &accepts}
	port := l.Addr().(*net.TCPAddr).Port

	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
		w.Write([]byte("ok"))
	})

	srv := &http.Server{Handler: mux}
	go srv.Serve(trackedL)
	defer srv.Close()

	// 2. Configure Vane
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l7Conf := advanced.ApplicationConfig{
		Pipeline: advanced.ProcessingStep{
			"internal.driver.upstream": advanced.PluginInstance{
				Input: map[string]interface{}{
					"url_prefix": fmt.Sprintf("http://127.0.0.1:%d", port),
				},
				Output: map[string]advanced.ProcessingStep{
					"success": {
						"internal.terminator.response": advanced.PluginInstance{
							Input: map[string]interface{}{},
						},
					},
				},
			},
		},
	}
	l7Bytes, _ := json.Marshal(l7Conf)
	sb.WriteConfig("application/httpx.json", l7Bytes)

	// L4+ Config: HTTP Resolver -> Upgrade to httpx
	l4pConf := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("httpx"),
	}
	l4pBytes, _ := json.Marshal(l4pConf)
	sb.WriteConfig("resolver/http.json", l4pBytes)

	// L4 Config: Upgrade to http (Plaintext L4+)
	l4Conf := advanced.L4FlowConfig{
		Connection: advanced.NewUpgrade("http"),
	}
	l4Bytes, _ := json.Marshal(l4Conf)
	sb.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	// 3. Start Vane
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	if err := proc.WaitForTcpPort(vanePort, 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Port failed to start", term.NewNode(err.Error())))
	}

	// 4. Send 5 Requests
	client := &http.Client{Timeout: 2 * time.Second}
	url := fmt.Sprintf("http://127.0.0.1:%d/", vanePort)

	for i := 0; i < 5; i++ {
		resp, err := client.Get(url)
		if err != nil {
			t.Fatal(term.FormatFailure(fmt.Sprintf("Request %d failed", i), term.NewNode(err.Error())))
		}
		io.Copy(io.Discard, resp.Body)
		resp.Body.Close()
		if resp.StatusCode != 200 {
			t.Fatal(term.FormatFailure(fmt.Sprintf("Request %d unexpected status: %d", i, resp.StatusCode), nil))
		}
		// Connection reuse might fail if we go too fast or if the server closes it.
		// Standard Go client reuses by default.
		time.Sleep(50 * time.Millisecond)
	}

	// 5. Verify Connection Count
	finalAccepts := atomic.LoadInt64(&accepts)
	if finalAccepts != 1 {
		t.Fatal(term.FormatFailure("Connection pooling failed", term.NewNode(fmt.Sprintf("Expected 1 connection, got %d. Logs:\n%s", finalAccepts, proc.DumpLogs()))))
	}
}
