/* test/integration/pkg/mock/tls_server.go */

package mock

import (
	"bufio"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"encoding/pem"
	"math/big"
	"net"
	"time"
)

type TlsServer struct {
	Listener net.Listener
	Port     int
	Quit     chan struct{}
}

// NewTlsEchoServer creates a TCP server wrapped in TLS.
// acceptedAlpn: list of ALPN protocols the server should accept/negotiate.
func NewTlsEchoServer(acceptedAlpn []string) (*TlsServer, error) {
	// 1. Generate Cert
	cert, err := generateTlsCert()
	if err != nil {
		return nil, err
	}

	config := &tls.Config{
		Certificates: []tls.Certificate{cert},
		NextProtos:   acceptedAlpn,
	}

	// 2. Listen
	l, err := tls.Listen("tcp", "127.0.0.1:0", config)
	if err != nil {
		return nil, err
	}

	s := &TlsServer{
		Listener: l,
		Port:     l.Addr().(*net.TCPAddr).Port,
		Quit:     make(chan struct{}),
	}

	go s.serve()
	return s, nil
}

func (s *TlsServer) serve() {
	for {
		select {
		case <-s.Quit:
			return
		default:
			conn, err := s.Listener.Accept()
			if err != nil {
				continue
			}
			go s.handle(conn)
		}
	}
}

func (s *TlsServer) handle(conn net.Conn) {
	defer conn.Close()
	// Perform handshake immediately to ensure connection is valid before echoing
	if tlsConn, ok := conn.(*tls.Conn); ok {
		if err := tlsConn.Handshake(); err != nil {
			return
		}
	}

	scanner := bufio.NewScanner(conn)
	for scanner.Scan() {
		line := scanner.Bytes()
		conn.Write(line)
		conn.Write([]byte("\n"))
	}
}

func (s *TlsServer) Close() {
	close(s.Quit)
	s.Listener.Close()
}

// Reuse logic to generate a simple cert
func generateTlsCert() (tls.Certificate, error) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return tls.Certificate{}, err
	}

	template := x509.Certificate{
		SerialNumber: big.NewInt(1),
		NotBefore:    time.Now().Add(-1 * time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
	}

	derBytes, err := x509.CreateCertificate(rand.Reader, &template, &template, &key.PublicKey, key)
	if err != nil {
		return tls.Certificate{}, err
	}
	certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: derBytes})
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(key)})

	return tls.X509KeyPair(certPEM, keyPEM)
}
