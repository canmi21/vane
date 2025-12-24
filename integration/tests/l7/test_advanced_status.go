/* integration/tests/l7/test_advanced_status.go */
package l7

import (
	"context"
	"fmt"
	"net/http"

	"canmi.net/vane-mock-tests/pkg/env"
)

// Define the common set of status scenarios
func getStatusScenarios() []Scenario {
	return []Scenario{
		{
			Name:         "Status 200 OK",
			ExpectStatus: http.StatusOK,
			RequestBody:  []byte("ok"),
		},
		{
			Name:         "Status 201 Created",
			ExpectStatus: http.StatusCreated,
			RequestBody:  []byte("created"),
		},
		{
			Name:         "Status 204 No Content",
			ExpectStatus: http.StatusNoContent,
			RequestBody:  []byte("should not be returned"),
			ExpectBody:   []byte{}, // Empty
		},
		{
			Name:         "Status 400 Bad Request",
			ExpectStatus: http.StatusBadRequest,
			RequestBody:  []byte("error details"),
		},
		{
			Name:         "Status 404 Not Found",
			ExpectStatus: http.StatusNotFound,
			RequestBody:  []byte("page not found"),
		},
		{
			Name:         "Status 500 Internal Error",
			ExpectStatus: http.StatusInternalServerError,
			RequestBody:  []byte("server crash trace"),
		},
		{
			Name:         "Status 418 I'm a teapot",
			ExpectStatus: http.StatusTeapot,
			RequestBody:  []byte("tea time"),
		},
	}
}

func TestStatusH2(ctx context.Context, s *env.Sandbox) error {
	scenarios := getStatusScenarios()
	// Test H2 -> H2
	if err := RunScenarios(ctx, s, ClientH2, UpstreamH2, scenarios); err != nil {
		return fmt.Errorf("H2->H2 Failed: %w", err)
	}
	return nil
}

func TestStatusH3(ctx context.Context, s *env.Sandbox) error {
	scenarios := getStatusScenarios()
	// Test H3 -> H3
	if err := RunScenarios(ctx, s, ClientH3, UpstreamH3, scenarios); err != nil {
		return fmt.Errorf("H3->H3 Failed: %w", err)
	}
	return nil
}
