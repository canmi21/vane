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
	Protocol string
}

// NewHttpUpstream creates a default echo server.
func NewHttpUpstream() (*HttpUpstream, error) {
	return NewHttpUpstreamWithHandler(nil)
}

// NewHttpUpstreamWithHandler allows custom handler logic.
func NewHttpUpstreamWithHandler(customHandler http.HandlerFunc) (*HttpUpstream, error) {
	tlsConf, err := GenerateTLSConfig([]string{"h2", "http/1.1"})
	if err != nil {
		return nil, err
	}

	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	tlsListener := tls.NewListener(l, tlsConf)

	mux := http.NewServeMux()
	if customHandler != nil {
		mux.HandleFunc("/", customHandler)
	} else {
		// Default Echo Handler
		mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("X-Upstream-Proto", r.Proto)
			body, _ := io.ReadAll(r.Body)
			w.Write(body)
		})
	}

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
