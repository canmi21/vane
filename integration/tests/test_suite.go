/* integration/tests/test_suite.go */
package tests

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/tests/l4"
)

// Re-define here to match runner
type TestFunc func(ctx context.Context, s *env.Sandbox) error

type TestCase struct {
	ID          int
	Name        string
	Description string
	Run         TestFunc
}

var Registry []TestCase

func Initialize() {
	// Import L4 tests
	for _, t := range l4.GetTests() {
		register(t.Name, t.Desc, TestFunc(t.Run))
	}
	// Future: l7.GetTests()...
}

func register(name, desc string, fn TestFunc) {
	// ID is assigned sequentially based on registration order.
	// This ID is permanent for the session, regardless of filtering.
	id := len(Registry) + 1
	Registry = append(Registry, TestCase{
		ID:          id,
		Name:        name,
		Description: desc,
		Run:         fn,
	})
}
