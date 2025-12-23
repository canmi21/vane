/* integration/tests/l7/test_advanced_body.go */
package l7

import (
	"context"

	"canmi.net/vane-mock-tests/pkg/env"
)

func getBodyScenarios() []Scenario {
	// Generate a 64KB binary blob
	blob64k := make([]byte, 64*1024)
	for i := range blob64k {
		blob64k[i] = byte(i % 256)
	}

	return []Scenario{
		{
			Name:         "JSON Payload",
			ExpectStatus: 200,
			RequestHeaders: map[string]string{
				"Content-Type": "application/json",
			},
			RequestBody: []byte(`{"user_id": 123, "name": "Vane Proxy", "roles": ["admin", "proxy"]}`),
		},
		{
			Name:         "HTML Payload",
			ExpectStatus: 200,
			RequestHeaders: map[string]string{
				"Content-Type": "text/html",
			},
			RequestBody: []byte(`<!DOCTYPE html><html><body><h1>Hello Vane</h1></body></html>`),
		},
		{
			Name:         "Binary Blob 64KB",
			ExpectStatus: 200,
			RequestHeaders: map[string]string{
				"Content-Type": "application/octet-stream",
			},
			RequestBody: blob64k,
		},
		{
			Name:         "Empty Body",
			ExpectStatus: 200,
			RequestBody:  []byte{},
		},
	}
}

func TestBodyH2(ctx context.Context, s *env.Sandbox) error {
	return RunScenarios(ctx, s, ClientH2, UpstreamH2, getBodyScenarios())
}

func TestBodyH3(ctx context.Context, s *env.Sandbox) error {
	return RunScenarios(ctx, s, ClientH3, UpstreamH3, getBodyScenarios())
}
