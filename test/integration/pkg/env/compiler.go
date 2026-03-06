/* test/integration/pkg/env/compiler.go */

package env

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
)

// CompileCgiBin finds the source C file, compiles it into the sandbox temp dir,
// and returns the absolute path to the executable.
func (s *Sandbox) CompileCgiBin(sourceRelativePath string) (string, error) {
	// 1. Resolve Project Root
	// We assume the test runs from integration/ dir.
	// Source: integration/tests/l7/cgi-bin/sample_bin.c

	// Try finding it relative to CWD
	cwd, _ := os.Getwd()
	sourcePath := filepath.Join(cwd, sourceRelativePath)

	if _, err := os.Stat(sourcePath); os.IsNotExist(err) {
		return "", fmt.Errorf("c source not found at %s", sourcePath)
	}

	// 2. Output Path inside Sandbox
	outPath := filepath.Join(s.RootDir, "cgi_test_bin")

	// 3. Compile
	cmd := exec.Command("cc", "-o", outPath, sourcePath)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("compilation failed: %v\n%s", err, string(output))
	}

	return outPath, nil
}

// CompileGoBin writes Go source code to a file and compiles it.
func (s *Sandbox) CompileGoBin(sourceContent string) (string, error) {
	srcPath := filepath.Join(s.RootDir, "plugin.go")
	if err := os.WriteFile(srcPath, []byte(sourceContent), 0644); err != nil {
		return "", fmt.Errorf("failed to write go source: %w", err)
	}

	// Vane security requires plugins to be in config/bin
	binDir := filepath.Join(s.ConfigDir, "bin")
	if err := os.MkdirAll(binDir, 0755); err != nil {
		return "", fmt.Errorf("failed to create trusted bin dir: %w", err)
	}

	outPath := filepath.Join(binDir, "plugin_bin")
	cmd := exec.Command("go", "build", "-o", outPath, srcPath)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("go compilation failed: %v\n%s", err, string(output))
	}

	return outPath, nil
}
