/* test/integration/pkg/mock/tcp_server.go */

package mock

import (
	"bufio"
	"net"
	"sync"
)

type TcpServer struct {
	Listener net.Listener
	Port     int
	wg       sync.WaitGroup
}

// NewTcpEchoServer creates a server that reads a line and echoes it back.
func NewTcpEchoServer() (*TcpServer, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return NewTcpEchoServerFromListener(l), nil
}

// NewTcpEchoServerFromListener creates a server from an existing listener.
func NewTcpEchoServerFromListener(l net.Listener) *TcpServer {
	s := &TcpServer{
		Listener: l,
		Port:     l.Addr().(*net.TCPAddr).Port,
	}

	s.wg.Add(1)
	go s.serve()
	return s
}

func (s *TcpServer) serve() {
	defer s.wg.Done()
	for {
		conn, err := s.Listener.Accept()
		if err != nil {
			// Listener closed
			return
		}
		go s.handle(conn)
	}
}

func (s *TcpServer) handle(conn net.Conn) {
	defer conn.Close()
	scanner := bufio.NewScanner(conn)
	// Simple Echo: Read line, Write line
	for scanner.Scan() {
		line := scanner.Bytes()
		// Write back exactly what was received plus a newline to frame it
		if _, err := conn.Write(line); err != nil {
			return
		}
		if _, err := conn.Write([]byte("\n")); err != nil {
			return
		}
	}
}

func (s *TcpServer) Close() {
	s.Listener.Close()
	s.wg.Wait()
}
