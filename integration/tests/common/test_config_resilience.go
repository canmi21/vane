/* integration/tests/common/test_config_resilience.go */
package common

import (
	"context"
	"time"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestConfigResilience(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane with an empty but valid configuration first
	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()

	// 2. Scenario A: Corrupted Syntax (Garbage data)
	// We write a file that is not even valid YAML
	garbageContent := []byte("!!INVALID!! ---\n  - [ } : this is garbage")
	if err := s.WriteConfig("listener/[80]/tcp.yaml", garbageContent); err != nil {
		return err
	}

	// Expect Vane to log an error about parsing
	// Actual log: "Failed to parse config file ..."
	if err := proc.WaitForLog("Failed to parse config file", 5*time.Second); err != nil {
		return term.FormatFailure("Vane did not report error for garbage YAML", term.NewNode(err.Error()))
	}

	// 3. Scenario B: Valid Syntax, Invalid Logic (Validation failure)
	// We use an invalid name (with underscores) which we know fails validation
	invalidLogicContent := []byte(`
protocols:
  - name: "invalid_name_with_underscores"
    priority: 10
    detect: { method: "fallback", pattern: "any" }
    destination: { type: "forward", forward: { strategy: "random", targets: [] } }
`)
	if err := s.WriteConfig("listener/[81]/tcp.yaml", invalidLogicContent); err != nil {
		return err
	}

	// Expect Vane to log "✗ Validation failed"
	if err := proc.WaitForLog("Validation failed", 5*time.Second); err != nil {
		return term.FormatFailure("Vane did not report validation failure for underscores", term.NewNode(err.Error()))
	}

	// 4. Final Check: Ensure Vane is still running
	// If Vane crashed, WaitForLog would have failed with "process exited unexpectedly"
	// but we double check.
	// We'll try to reach the console port
	conn, err := s.ConnectConsole()
	if err != nil {
		return term.FormatFailure("Vane crashed or stopped responding after bad configs", term.NewNode(err.Error()))
	}
	conn.Close()

	return nil
}
