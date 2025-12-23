/* integration/pkg/mock/upstream_h3.go */
package mock

import (
	"fmt"
	"io"
	"net"
	"net/http"

	"github.com/quic-go/quic-go/http3"
)

type H3Upstream struct {
	Server *http3.Server
	Port   int
}

func NewH3Upstream() (*H3Upstream, error) {
	// 1. Generate TLS Config
	tlsConf, err := GenerateTLSConfig([]string{"h3"})
	if err != nil {
		return nil, err
	}

	// 2. Setup Handler
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Upstream-Proto", "HTTP/3.0")
		body, _ := io.ReadAll(r.Body)
		w.Write(body)
	})

	// 3. Listen on UDP port
	// http3.Server doesn't have a simple "ListenAddr" that returns the port easily before blocking.
	// We manually create a UDP conn to get a port, then close it, then let Server listen.
	// NOTE: Small race condition window, but acceptable for tests.
	udpAddr, err := net.ResolveUDPAddr("udp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	udpConn, err := net.ListenUDP("udp", udpAddr)
	if err != nil {
		return nil, err
	}
	port := udpConn.LocalAddr().(*net.UDPAddr).Port
	udpConn.Close()

	srv := &http3.Server{
		Addr:      fmt.Sprintf("127.0.0.1:%d", port),
		Handler:   mux,
		TLSConfig: tlsConf,
	}

	h3 := &H3Upstream{
		Server: srv,
		Port:   port,
	}

	go func() {
		if err := srv.ListenAndServe(); err != nil {
			// Expected error on Close()
		}
	}()

	return h3, nil
}

func (s *H3Upstream) Close() {
	s.Server.Close()
}
