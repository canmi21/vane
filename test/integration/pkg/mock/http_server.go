/* test/integration/pkg/mock/http_server.go */

package mock

import (
	"io"
	"net"
	"net/http"
)

type HttpServer struct {
	Listener net.Listener
	Port     int
	Server   *http.Server
}

// NewHttpEchoServer creates a standard HTTP/1.1 server.
// It echoes the request body and adds a header.
func NewHttpEchoServer() (*HttpServer, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		// Echo Header
		w.Header().Set("X-Mock-Server", "Go-Std-Lib")

		// Echo Body
		body, _ := io.ReadAll(r.Body)
		w.Write(body)
	})

	srv := &http.Server{Handler: mux}

	s := &HttpServer{
		Listener: l,
		Port:     l.Addr().(*net.TCPAddr).Port,
		Server:   srv,
	}

	go srv.Serve(l)
	return s, nil
}

func (s *HttpServer) Close() {
	s.Server.Close()
	s.Listener.Close()
}
