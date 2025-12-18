/* integration/pkg/env/sandbox.go */
package env

import (
	"fmt"
	"os"
	"path/filepath"
)

// Sandbox represents an isolated execution environment for a Vane instance.
type Sandbox struct {
	ID          string // Random ID for the run
	RootDir     string // /tmp/vane_test_xyz
	ConfigDir   string // /tmp/vane_test_xyz/config
	SocketDir   string // /tmp/vane_test_xyz/socket
	ConsolePort int    // TCP port for Vane Console/HealthCheck
}

// NewSandbox creates the directory structure and allocates a console port.
func NewSandbox() (*Sandbox, error) {
	// 1. Create a temporary directory prefix
	rootDir, err := os.MkdirTemp("", "vane_test_*")
	if err != nil {
		return nil, fmt.Errorf("failed to create temp root: %w", err)
	}

	// 2. Define sub-paths
	// Vane expects specific subdirectories inside CONFIG_DIR
	configDir := filepath.Join(rootDir, "config")
	socketDir := filepath.Join(rootDir, "socket")

	// 3. Create Directory Structure
	// Based on loader.rs requirements
	subDirs := []string{
		"listener",
		"application",
		"resolver",
		"certs",
	}

	for _, sub := range subDirs {
		path := filepath.Join(configDir, sub)
		if err := os.MkdirAll(path, 0755); err != nil {
			os.RemoveAll(rootDir) // Cleanup on fail
			return nil, fmt.Errorf("failed to create config subdir %s: %w", sub, err)
		}
	}

	// Also ensure socket dir exists
	if err := os.MkdirAll(socketDir, 0755); err != nil {
		os.RemoveAll(rootDir)
		return nil, fmt.Errorf("failed to create socket dir: %w", err)
	}

	// 4. Allocate Console Port
	port, err := GetFreePort()
	if err != nil {
		os.RemoveAll(rootDir)
		return nil, fmt.Errorf("failed to allocate console port: %w", err)
	}

	return &Sandbox{
		ID:          filepath.Base(rootDir),
		RootDir:     rootDir,
		ConfigDir:   configDir,
		SocketDir:   socketDir,
		ConsolePort: port,
	}, nil
}

// Cleanup removes all temporary files.
func (s *Sandbox) Cleanup() {
	if s.RootDir != "" {
		os.RemoveAll(s.RootDir)
	}
}

// WriteFile writes configuration content to the sandbox.
// relativePath example: "application/httpx.yaml"
func (s *Sandbox) WriteConfig(relativePath string, content []byte) error {
	fullPath := filepath.Join(s.ConfigDir, relativePath)
	dir := filepath.Dir(fullPath)

	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	return os.WriteFile(fullPath, content, 0644)
}
