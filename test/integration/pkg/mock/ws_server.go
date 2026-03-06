/* test/integration/pkg/mock/ws_server.go */

package mock

import (
	"crypto/sha1"
	"encoding/base64"
	"io"
	"net"
	"net/http"
)

// WSEchoHandler is a manual WebSocket handshake handler.
// It upgrades the connection and echoes raw TCP data.
func WSEchoHandler(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Connection") != "Upgrade" || r.Header.Get("Upgrade") != "websocket" {
		w.WriteHeader(400)
		return
	}

	hj, ok := w.(http.Hijacker)
	if !ok {
		w.WriteHeader(500)
		return
	}
	conn, bufrw, err := hj.Hijack()
	if err != nil {
		return
	}
	defer conn.Close()

	// Compute Sec-WebSocket-Accept
	key := r.Header.Get("Sec-WebSocket-Key")
	accept := computeAcceptKey(key)

	// Send 101 Response
	// Note: We are now in raw TCP mode
	bufrw.WriteString("HTTP/1.1 101 Switching Protocols\r\n")
	bufrw.WriteString("Upgrade: websocket\r\n")
	bufrw.WriteString("Connection: Upgrade\r\n")
	bufrw.WriteString("Sec-WebSocket-Accept: " + accept + "\r\n")
	bufrw.WriteString("\r\n")
	bufrw.Flush()

	// Echo Loop (Raw Bytes)
	// Since Vane is a transparent tunnel, we don't strictly need to parse WS frames for the 1GB test.
	// We just treat it as a TCP stream.
	io.Copy(conn, conn)
}

func computeAcceptKey(key string) string {
	h := sha1.New()
	h.Write([]byte(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
	return base64.StdEncoding.EncodeToString(h.Sum(nil))
}

// NewWsUpstream creates an HTTP server that supports WS upgrade echo.
func NewWsUpstream() (*HttpServer, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/ws", WSEchoHandler)

	srv := &http.Server{Handler: mux}

	s := &HttpServer{
		Listener: l,
		Port:     l.Addr().(*net.TCPAddr).Port,
		Server:   srv,
	}

	go srv.Serve(l)
	return s, nil
}
