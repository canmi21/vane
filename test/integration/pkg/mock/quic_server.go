/* test/integration/pkg/mock/quic_server.go */

package mock

import (
	"context"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"encoding/pem"
	"math/big"
	"net"
	"time"

	quic "github.com/quic-go/quic-go"
)

type QuicServer struct {
	Listener *quic.Listener
	Port     int
	Quit     chan struct{}
}

// NewQuicEchoServer creates a QUIC server that accepts streams and echoes data.
func NewQuicEchoServer() (*QuicServer, error) {
	// 1. Generate ephemeral TLS config
	tlsConf, err := generateQuicTLSConfig()
	if err != nil {
		return nil, err
	}
	tlsConf.NextProtos = []string{"h3", "quic-echo"} // Support ALPNs

	// ListenAddr
	listener, err := quic.ListenAddr("127.0.0.1:0", tlsConf, nil)
	if err != nil {
		return nil, err
	}

	udpAddr := listener.Addr().(*net.UDPAddr)

	s := &QuicServer{
		Listener: listener,
		Port:     udpAddr.Port,
		Quit:     make(chan struct{}),
	}

	go s.serve()
	return s, nil
}

func (s *QuicServer) serve() {
	for {
		ctx := context.Background()
		conn, err := s.Listener.Accept(ctx)
		if err != nil {
			// Listener closed
			return
		}
		go s.handleConn(conn)
	}
}

// quic.Connection -> *quic.Conn
func (s *QuicServer) handleConn(conn *quic.Conn) {
	for {
		// Accept bidirectional streams
		stream, err := conn.AcceptStream(context.Background())
		if err != nil {
			return
		}
		// Simple Echo: Copy input to output
		go func() {
			defer stream.Close()
			buf := make([]byte, 1024)
			for {
				n, err := stream.Read(buf)
				if err != nil {
					return
				}
				stream.Write(buf[:n])
			}
		}()
	}
}

func (s *QuicServer) Close() {
	close(s.Quit)
	s.Listener.Close()
}

// Helper: Generate self-signed cert for QUIC
func generateQuicTLSConfig() (*tls.Config, error) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		return nil, err
	}

	template := x509.Certificate{
		SerialNumber: big.NewInt(1),
		NotBefore:    time.Now().Add(-1 * time.Hour),
		NotAfter:     time.Now().Add(24 * time.Hour),
	}

	derBytes, err := x509.CreateCertificate(rand.Reader, &template, &template, &key.PublicKey, key)
	if err != nil {
		return nil, err
	}
	certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: derBytes})
	keyPEM := pem.EncodeToMemory(&pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(key)})

	tlsCert, err := tls.X509KeyPair(certPEM, keyPEM)
	if err != nil {
		return nil, err
	}
	return &tls.Config{
		Certificates: []tls.Certificate{tlsCert},
		NextProtos:   []string{"quic-echo"},
	}, nil
}
