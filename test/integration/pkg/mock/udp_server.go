/* test/integration/pkg/mock/udp_server.go */

package mock

import (
	"net"
)

type UdpServer struct {
	Conn     *net.UDPConn
	Port     int
	Response []byte
	Quit     chan struct{}
}

// NewUdpFixedResponseServer creates a server that replies with a fixed byte slice.
func NewUdpFixedResponseServer(response []byte) (*UdpServer, error) {
	addr, err := net.ResolveUDPAddr("udp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}

	conn, err := net.ListenUDP("udp", addr)
	if err != nil {
		return nil, err
	}

	s := &UdpServer{
		Conn:     conn,
		Port:     conn.LocalAddr().(*net.UDPAddr).Port,
		Response: response,
		Quit:     make(chan struct{}),
	}

	go s.serve()
	return s, nil
}

func (s *UdpServer) serve() {
	buf := make([]byte, 2048)
	for {
		select {
		case <-s.Quit:
			return
		default:
			// Non-blocking read trick not used here for simplicity; relies on Close() to unblock
			n, remoteAddr, err := s.Conn.ReadFromUDP(buf)
			if err != nil {
				return
			}
			if n > 0 {
				s.Conn.WriteToUDP(s.Response, remoteAddr)
			}
		}
	}
}

func (s *UdpServer) Close() {
	close(s.Quit)
	s.Conn.Close()
}
