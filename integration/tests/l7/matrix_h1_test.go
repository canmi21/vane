/* integration/tests/l7/matrix_h1_test.go */
package l7

import (
	"testing"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH1toH1(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH1, UpstreamH1); err != nil {
		t.Fatal(err)
	}
}

func TestH1toH2(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH1, UpstreamH2); err != nil {
		t.Fatal(err)
	}
}

func TestH1toH3(t *testing.T) {
	sb, ctx := env.SetupTest(t)
	if err := RunMatrixTest(ctx, sb, ClientH1, UpstreamH3); err != nil {
		t.Fatal(err)
	}
}
