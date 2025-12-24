/* integration/pkg/env/compiler.go */
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
		return "", fmt.Errorf("C source not found at %s", sourcePath)
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
