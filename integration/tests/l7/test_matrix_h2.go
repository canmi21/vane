/* integration/tests/l7/test_matrix_h2.go */
package l7

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH2toH1(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH2, UpstreamH1)
}

func TestH2toH2(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH2, UpstreamH2)
}

func TestH2toH3(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH2, UpstreamH3)
}
