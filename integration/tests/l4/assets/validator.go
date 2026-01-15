/* integration/tests/l4/assets/validator.go */

package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"regexp"
)

type MiddlewareOutput struct {
	Branch string                 `json:"branch"`
	Store  map[string]interface{} `json:"store,omitempty"`
}

func writeResult(msg string) {
	path := os.Getenv("VALIDATOR_OUTPUT_FILE")
	if path == "" {
		return
	}
	os.WriteFile(path, []byte(msg), 0644)
}

func main() {
	// 1. Read Input from Stdin
	inputBytes, err := io.ReadAll(os.Stdin)
	if err != nil {
		fatal("Failed to read stdin: %v", err)
	}
	// ... (rest of parsing logic) ...
	var inputs map[string]interface{}
	if err := json.Unmarshal(inputBytes, &inputs); err != nil {
		fatal("Failed to parse input JSON: %v. Input: %s", err, string(inputBytes))
	}

	// 2. Validate Fields
	if err := validateIP(inputs["conn_ip"]); err != nil {
		fatal("conn.ip invalid: %v", err)
	}

	if err := validatePort(inputs["conn_port"]); err != nil {
		fatal("conn.port invalid: %v", err)
	}

	if val, ok := inputs["conn_proto"].(string); !ok || (val != "tcp" && val != "udp") {
		fatal("conn.proto invalid: expected tcp/udp, got %v", inputs["conn_proto"])
	}

	if val, ok := inputs["conn_uuid"].(string); !ok || len(val) < 30 {
		fatal("conn.uuid invalid: %v", inputs["conn_uuid"])
	}

	if err := validateTimestamp(inputs["conn_timestamp"]); err != nil {
		fatal("conn.timestamp invalid: %v", err)
	}

	if err := validateIP(inputs["server_ip"]); err != nil {
		fatal("server.ip invalid: %v", err)
	}

	if err := validatePort(inputs["server_port"]); err != nil {
		fatal("server.port invalid: %v", err)
	}

	// 3. Output Success
	writeResult("SUCCESS")
	output := MiddlewareOutput{
		Branch: "success",
	}
	outBytes, _ := json.Marshal(output)
	os.Stdout.Write(outBytes)
}

func fatal(format string, args ...interface{}) {
	msg := fmt.Sprintf(format, args...)
	writeResult("FAILURE: " + msg)
	fmt.Fprintf(os.Stderr, "%s\n", msg)

	// Return failure branch
	output := MiddlewareOutput{
		Branch: "failure",
	}
	outBytes, _ := json.Marshal(output)
	os.Stdout.Write(outBytes)
	os.Exit(1)
}
func validateIP(v interface{}) error {
	s, ok := v.(string)
	if !ok {
		return fmt.Errorf("not a string: %v", v)
	}
	if ip := net.ParseIP(s); ip == nil {
		return fmt.Errorf("malformed IP: %s", s)
	}
	return nil
}

func validatePort(v interface{}) error {
	// Port might come as string or number depending on template output
	s, ok := v.(string)
	if !ok {
		// try float64 (json default for numbers)
		if f, ok := v.(float64); ok {
			if f < 0 || f > 65535 {
				return fmt.Errorf("port out of range: %f", f)
			}
			return nil
		}
		return fmt.Errorf("not a string or number: %v", v)
	}

	// Validate string port
	matched, _ := regexp.MatchString(`^\d+$`, s)
	if !matched {
		return fmt.Errorf("port not a number: %s", s)
	}
	return nil
}

func validateTimestamp(v interface{}) error {
	s, ok := v.(string)
	if !ok {
		return fmt.Errorf("not a string: %v", v)
	}
	// Check if it looks like a number
	matched, _ := regexp.MatchString(`^\d+$`, s)
	if !matched {
		return fmt.Errorf("not a timestamp: %s", s)
	}
	return nil
}