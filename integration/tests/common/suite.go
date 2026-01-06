/* integration/tests/common/suite.go */
package common

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

type TestFunc func(ctx context.Context, s *env.Sandbox) error

type TestCase struct {
	Name string
	Desc string
	Run  TestFunc
}

func GetTests() []TestCase {
	return []TestCase{
		{Name: "common_test_no_console", Desc: "Verifies no-console mode without ACCESS_TOKEN", Run: TestNoConsole},
		{Name: "common_test_config_resilience", Desc: "Verifies Vane stability with bad configs", Run: TestConfigResilience},
		{Name: "common_test_flow_timeout", Desc: "Flow: Execution Timeout Protection", Run: TestFlowTimeout},
		{Name: "common_test_circuit_breaker", Desc: "Flow: External Plugin Circuit Breaker", Run: TestExternalCircuitBreaker},
		{Name: "common_test_config_hot_reload", Desc: "Config: Hot Reload & Keep-Last-Known-Good", Run: TestConfigHotReload},
	}
}
