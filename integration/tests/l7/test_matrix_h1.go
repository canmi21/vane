/* integration/tests/l7/test_matrix_h1.go */
package l7

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

func TestH1toH1(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH1, UpstreamH1)
}

func TestH1toH2(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH1, UpstreamH2)
}

func TestH1toH3(ctx context.Context, s *env.Sandbox) error {
	return RunMatrixTest(ctx, s, ClientH1, UpstreamH3)
}
