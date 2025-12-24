/* integration/tests/l7/test_matrix_h3.go */
package l7

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH3toH1(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH3, UpstreamH1)
}

func TestH3toH2(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH3, UpstreamH2)
}

func TestH3toH3(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH3, UpstreamH3)
}
