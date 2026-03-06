/* integration/tests/l7/matrix_h3_test.go */
package l7

import (
	"testing"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH3toH1(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH3, UpstreamH1); err != nil {
		t.Fatal(err)
	}
}

func TestH3toH2(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH3, UpstreamH2); err != nil {
		t.Fatal(err)
	}
}

func TestH3toH3(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH3, UpstreamH3); err != nil {
		t.Fatal(err)
	}
}
