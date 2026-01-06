/* integration/tests/l7/test_static_server.go */
package l7

import (
	"bytes"
	"compress/gzip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"canmi.net/vane-mock-tests/pkg/config/advanced"
	"canmi.net/vane-mock-tests/pkg/env"
	"canmi.net/vane-mock-tests/pkg/term"
)

func setupStaticTest(s *env.Sandbox, extraInputs map[string]interface{}) (int, string, error) {
	// 1. Create Temp Root
	tmpRoot := filepath.Join(s.RootDir, "static_content")
	os.MkdirAll(tmpRoot, 0755)

	// 2. Create index.html
	os.WriteFile(filepath.Join(tmpRoot, "index.html"), []byte("<h1>Hello Vane</h1>"), 0644)
	// 3. Create a larger file for range requests
	largeContent := strings.Repeat("ABCDEFGHIJ", 10) // 100 bytes
	os.WriteFile(filepath.Join(tmpRoot, "large.txt"), []byte(largeContent), 0644)

	// 4. Configure Vane
	ports, _ := env.GetFreePorts(1)
	vanePort := ports[0]

	inputs := map[string]interface{}{
		"root": tmpRoot,
		"uri":  "{{req.path}}",
	}
	for k, v := range extraInputs {
		inputs[k] = v
	}

	l7Conf := advanced.ApplicationConfig{
		Pipeline: advanced.ProcessingStep{
			"internal.driver.static": advanced.PluginInstance{
				Input: inputs,
				Output: map[string]advanced.ProcessingStep{
					"success": {
						"internal.terminator.response": advanced.PluginInstance{
							Input: map[string]interface{}{},
						},
					},
					"not_found": {
						"internal.terminator.response": advanced.PluginInstance{
							Input: map[string]interface{}{"status": 404, "body": "Not Found Custom"},
						},
					},
					"failure": {
						"internal.terminator.response": advanced.PluginInstance{
							Input: map[string]interface{}{"status": 500, "body": "Static Error"},
						},
					},
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

	return vanePort, tmpRoot, nil
}

func TestStaticServeBasic(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	vanePort, _, _ := setupStaticTest(s, nil)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}

	// 1. Request / (should get index.html)
	resp, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/", vanePort))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if !strings.Contains(string(body), "Hello Vane") {
		return term.FormatFailure("Failed to serve index.html", term.NewNode(string(body)))
	}

	// 2. Request /large.txt
	resp2, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/large.txt", vanePort))
	if err != nil {
		return err
	}
	defer resp2.Body.Close()
	if resp2.StatusCode != 200 || resp2.ContentLength != 100 {
		return term.FormatFailure("Failed to serve large.txt", nil)
	}

	return nil
}

func TestStaticRange(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	vanePort, _, _ := setupStaticTest(s, nil)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}

	// Request Range: bytes=10-19
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/large.txt", vanePort), nil)
	req.Header.Set("Range", "bytes=10-19")
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != 206 {
		return term.FormatFailure(fmt.Sprintf("Expected 206, got %d", resp.StatusCode), nil)
	}
	body, _ := io.ReadAll(resp.Body)
	if len(body) != 10 {
		return term.FormatFailure(fmt.Sprintf("Expected 10 bytes, got %d", len(body)), nil)
	}
	// "ABCDEFGHIJ" repeated. 10-19 should be "ABCDEFGHIJ" (the second block)
	if string(body) != "ABCDEFGHIJ" {
		return term.FormatFailure(fmt.Sprintf("Wrong range content: %s", string(body)), nil)
	}

	return nil
}

func TestStaticSPA(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	// Enable SPA mode
	vanePort, _, _ := setupStaticTest(s, map[string]interface{}{"spa": true})

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}

	// Request non-existent path
	resp, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/some/missing/route", vanePort))
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	// Should get 200 OK and index.html content
	if resp.StatusCode != 200 {
		return term.FormatFailure(fmt.Sprintf("SPA: Expected 200, got %d", resp.StatusCode), nil)
	}
	body, _ := io.ReadAll(resp.Body)
	if !strings.Contains(string(body), "Hello Vane") {
		return term.FormatFailure("SPA: Failed to fallback to index.html", term.NewNode(string(body)))
	}

	return nil
}

func TestStaticBrowse(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	// Enable browsing and delete index.html to force listing
	vanePort, root, _ := setupStaticTest(s, map[string]interface{}{"browse": true})
	os.Remove(filepath.Join(root, "index.html"))

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

	if resp.StatusCode != 200 {
		return term.FormatFailure(fmt.Sprintf("Browse: Expected 200, got %d", resp.StatusCode), nil)
	}
	body, _ := io.ReadAll(resp.Body)
	// Check for a link to large.txt
	if !strings.Contains(string(body), "large.txt") || !strings.Contains(string(body), "<a href=") {
		return term.FormatFailure("Browse: Listing HTML missing file links", term.NewNode(string(body)))
	}

	return nil
}

func TestStaticTraversal(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	vanePort, _, _ := setupStaticTest(s, nil)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	client := &http.Client{Timeout: 2 * time.Second}

	// Try traversal
	resp, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/../../etc/passwd", vanePort))
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	// Should either be 404 (not found in jail) or 500 (plugin returned failure branch)
	// Actually, router::resolve_path returns Err for traversal, which triggers 'failure' branch.
	// Our config maps 'failure' to 500.
	if resp.StatusCode == 200 {
		return term.FormatFailure("Directory traversal succeeded (vulnerability!)", nil)
	}

	return nil
}

func TestStaticPrecompressed(ctx context.Context, s *env.Sandbox) error {
	debug, _ := ctx.Value(env.DebugKey).(bool)
	// Enable precompressed mode
	vanePort, root, _ := setupStaticTest(s, map[string]interface{}{"precompress": true})

	// Create dummy files
	os.WriteFile(filepath.Join(root, "style.css"), []byte("body { color: red; }"), 0644)

	// Create REAL .gz content
	var b bytes.Buffer
	gw := gzip.NewWriter(&b)
	gw.Write([]byte("GZIP_MOCK_CONTENT"))
	gw.Close()
	os.WriteFile(filepath.Join(root, "style.css.gz"), b.Bytes(), 0644)

	proc, err := s.StartVane(ctx, debug)
	if err != nil {
		return err
	}
	defer proc.Stop()
	proc.WaitForTcpPort(vanePort, 5*time.Second)

	// Create a client that does NOT auto-decompress
	tr := &http.Transport{
		DisableCompression: true,
	}
	client := &http.Client{
		Timeout:   2 * time.Second,
		Transport: tr,
	}

	// 1. Request with Accept-Encoding: gzip
	req, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/style.css", vanePort), nil)
	req.Header.Set("Accept-Encoding", "gzip, deflate")
	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return term.FormatFailure(fmt.Sprintf("Expected 200, got %d", resp.StatusCode), nil)
	}

	// Verify Header
	if ce := resp.Header.Get("Content-Encoding"); ce != "gzip" {
		return term.FormatFailure(fmt.Sprintf("Expected Content-Encoding: gzip, got '%s'", ce), nil)
	}

	// Verify Body (Should be RAW gzip bytes because we disabled compression)
	body, _ := io.ReadAll(resp.Body)
	if !bytes.Equal(body, b.Bytes()) {
		return term.FormatFailure("Did not serve correct .gz bytes", nil)
	}

	// 2. Request WITHOUT Accept-Encoding
	// Need a new client or reuse? Reuse is fine, DisableCompression just stops automatic header addition and decompression.
	// But standard client behavior adds "Accept-Encoding: gzip" automatically.
	// DisableCompression: true prevents that.

	req2, _ := http.NewRequest("GET", fmt.Sprintf("http://127.0.0.1:%d/style.css", vanePort), nil)
	resp2, err := client.Do(req2)
	if err != nil {
		return err
	}
	defer resp2.Body.Close()

	// Verify Header
	if ce := resp2.Header.Get("Content-Encoding"); ce != "" {
		return term.FormatFailure(fmt.Sprintf("Expected no Content-Encoding, got '%s'", ce), nil)
	}
	// Verify Body (should serve original)
	body2, _ := io.ReadAll(resp2.Body)
	if string(body2) != "body { color: red; }" {
		return term.FormatFailure("Did not serve original content", term.NewNode(string(body2)))
	}

	return nil
}
