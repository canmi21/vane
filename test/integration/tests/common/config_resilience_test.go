/* test/integration/tests/common/config_resilience_test.go */

package common

import (
	"testing"
	"time"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func TestConfigResilience(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// 1. Start Vane with an empty but valid configuration first
	proc, err := sb.StartVane(ctx, debug)
	if err != nil {
		t.Fatal(err)
	}
	defer proc.Stop()

	// 2. Scenario A: Corrupted Syntax (Garbage data)
	// We write a file that is not even valid YAML
	garbageContent := []byte("!!INVALID!! ---\n  - [ } : this is garbage")
	if err := sb.WriteConfig("listener/[80]/tcp.yaml", garbageContent); err != nil {
		t.Fatal(err)
	}

	// The watcher detects the change and rescans the directory.
	// Individual parse failures are recorded in ScanResult::failed and now
	// reported via the on_error callback in Vane, which logs a warning.
	if err := proc.WaitForLog("New TCP config is invalid", 5*time.Second); err != nil {
		t.Fatal(term.FormatFailure("Vane did not detect garbage config change", term.NewNode(err.Error())))
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
	if err := sb.WriteConfig("listener/[81]/tcp.yaml", invalidLogicContent); err != nil {
		t.Fatal(err)
	}

	// Same as above: watcher fires, validation failure is silent (ScanResult::failed).
	// Wait for the watcher to process the change.
	time.Sleep(1 * time.Second)

	// 4. Final Check: Ensure Vane is still running after both bad configs
	conn, err := sb.ConnectConsole()
	if err != nil {
		t.Fatal(term.FormatFailure("Vane crashed or stopped responding after bad configs", term.NewNode(err.Error())))
	}
	conn.Close()
}
