/* integration/pkg/mock/upstream_http.go */
package mock

import (
	"crypto/tls"
	"io"
	"net"
	"net/http"
)

type HttpUpstream struct {
	Listener net.Listener
	Server   *http.Server
	Port     int
	Protocol string // "h1" or "h2" (negotiated)
}

// NewHttpUpstream creates a server supporting HTTPS (H1 and H2).
func NewHttpUpstream() (*HttpUpstream, error) {
	// 1. Generate TLS Config with H2 support
	tlsConf, err := GenerateTLSConfig([]string{"h2", "http/1.1"})
	if err != nil {
		return nil, err
	}

	// 2. Setup Listener
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	tlsListener := tls.NewListener(l, tlsConf)

	// 3. Setup Handler
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Upstream-Proto", r.Proto)
		w.Header().Set("X-Upstream-Method", r.Method)

		body, _ := io.ReadAll(r.Body)
		w.Write(body)
	})

	srv := &http.Server{Handler: mux}

	s := &HttpUpstream{
		Listener: tlsListener,
		Server:   srv,
		Port:     l.Addr().(*net.TCPAddr).Port,
	}

	go srv.Serve(tlsListener)

	return s, nil
}

func (s *HttpUpstream) Close() {
	s.Server.Close()
	s.Listener.Close()
}
