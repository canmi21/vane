/* integration/tests/l7/matrix_h2_test.go */
package l7

import (
	"testing"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH2toH1(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH2, UpstreamH1); err != nil {
		t.Fatal(err)
	}
}

func TestH2toH2(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH2, UpstreamH2); err != nil {
		t.Fatal(err)
	}
}

func TestH2toH3(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH2, UpstreamH3); err != nil {
		t.Fatal(err)
	}
}
