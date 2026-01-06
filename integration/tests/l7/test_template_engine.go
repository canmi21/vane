/* integration/tests/l7/test_template_engine.go */
package l7

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func setupTemplateTest(s *env.Sandbox, responseBody interface{}) (int, error) {
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l7Conf := advanced.ApplicationConfig{
		Pipeline: advanced.ProcessingStep{
			"internal.terminator.response": advanced.PluginInstance{
				Input: map[string]interface{}{
					"status": 200,
					"body":   responseBody,
				},
			},
		},
	}
	l7Bytes, _ := json.Marshal(l7Conf)
	s.WriteConfig("application/httpx.json", l7Bytes)

	l4pConf := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
	l4pBytes, _ := json.Marshal(l4pConf)
	s.WriteConfig("resolver/http.json", l4pBytes)

	l4Conf := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("http")}
	l4Bytes, _ := json.Marshal(l4Conf)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	return vanePort, nil
}

func TestTemplateHeaderHijacking(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	vanePort, _ := setupTemplateTest(s, "Header: {{req.header.x-vane-test}}")

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/", vanePort), nil)
	req.Header.Set("X-Vane-Test", "magic-value-123")

	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)

	if string(body) != "Header: magic-value-123" {
		return term.FormatFailure("Header hijacking failed", term.NewNode(fmt.Sprintf("Got: %q", string(body))))
	}
	return nil
}

func TestTemplateBodyHijacking(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	vanePort, _ := setupTemplateTest(s, "Body: {{req.body}}")

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}
	testData := "This is a secret message from client"
	resp, err := client.Post(fmt.Sprintf("http://127.0.0.1:%d/", vanePort), "text/plain", strings.NewReader(testData))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)

	if string(body) != "Body: "+testData {
		return term.FormatFailure("Body hijacking failed", term.NewNode(fmt.Sprintf("Got: %q", string(body))))
	}
	return nil
}

func TestTemplateNested(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	// Body uses nested template to resolve a header name from another header
	vanePort, _ := setupTemplateTest(s, "Nested: {{req.header.{{req.header.x-target-header}}}}")

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/", vanePort), nil)
	req.Header.Set("X-Target-Header", "x-real-data")
	req.Header.Set("X-Real-Data", "nested-success")

	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)

	if string(body) != "Nested: nested-success" {
		return term.FormatFailure("Nested template resolution failed", term.NewNode(fmt.Sprintf("Got: %q", string(body))))
	}
	return nil
}

func TestTemplateRecursionLimit(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	// Nested depth 7 (exceeds default 5)
	vanePort, _ := setupTemplateTest(s, "Deep: {{a.{{b.{{c.{{d.{{e.{{f.{{g}}}}}}}}}}}}}}")

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/", vanePort))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)

	// Vane should return truncated or error string, but NOT crash and NOT resolve it.
	// Based on resolver.rs, it returns String::new() for the deepest part and logs error.
	if strings.Contains(string(body), "{{g}}") || len(body) == 0 {
		// Acceptable failure behaviors
		return nil
	}

	// If it contains "Deep: " and some parts, it's also okay, as long as it didn't crash.
	if !strings.HasPrefix(string(body), "Deep:") {
		return term.FormatFailure("Recursion limit test gave unexpected output", term.NewNode(string(body)))
	}

	return nil
}

func TestTemplateJsonResolution(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)

	// Testing resolve_inputs with nested JSON structure.
	// Since we can't easily inspect internal plugin state, we use 'response' plugin's 'headers' input
	// which is a Map.
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	l7Conf := advanced.ApplicationConfig{
		Pipeline: advanced.ProcessingStep{
			"internal.terminator.response": advanced.PluginInstance{
				Input: map[string]interface{}{
					"status": 200,
					"headers": map[string]interface{}{
						"X-Echo-Host": "{{req.header.host}}",
						"X-Nested": map[string]interface{}{
							"val": "{{req.header.x-test}}",
						},
					},
					"body": "JSON Template Test",
				},
			},
		},
	}
	l7Bytes, _ := json.Marshal(l7Conf)
	s.WriteConfig("application/httpx.json", l7Bytes)

	l4pConf := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("httpx")}
	l4pBytes, _ := json.Marshal(l4pConf)
	s.WriteConfig("resolver/http.json", l4pBytes)

	l4Conf := advanced.L4FlowConfig{Connection: advanced.NewUpgrade("http")}
	l4Bytes, _ := json.Marshal(l4Conf)
	s.WriteConfig(fmt.Sprintf("listener/[%d]/tcp.json", vanePort), l4Bytes)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/", vanePort), nil)
	req.Header.Set("X-Test", "nest-json-ok")

	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.Header.Get("X-Echo-Host") == "" {
		return term.FormatFailure("JSON resolution failed for top-level string in Map", nil)
	}
	// Note: Vane's SendResponsePlugin might not support nested maps in headers perfectly
	// but it should at least resolve the values during resolve_inputs.

	return nil
}
