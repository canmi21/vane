/* test/integration/pkg/env/ports.go */

package env

import (
	"net"
)

// GetFreePort asks the kernel for a free TCP port.
// It creates a listener, extracts the port, and closes it immediately.
// Note: There is a tiny race condition window here, but usually safe for local tests.
func GetFreePort() (int, error) {
	addr, err := net.ResolveTCPAddr("tcp", "localhost:0")
	if err != nil {
		return 0, err
	}

	l, err := net.ListenTCP("tcp", addr)
	if err != nil {
		return 0, err
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port, nil
}

// GetFreePorts returns n distinct free ports.
func GetFreePorts(n int) ([]int, error) {
	var ports []int
	for i := 0; i < n; i++ {
		p, err := GetFreePort()
		if err != nil {
			return nil, err
		}
		ports = append(ports, p)
	}
	return ports, nil
}
