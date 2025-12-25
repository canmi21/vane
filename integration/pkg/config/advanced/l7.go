/* integration/pkg/config/advanced/l7.go */
package advanced

// NewFetchUpstream creates a ProcessingStep for internal.driver.upstream.
// It acts as an L7 Middleware/Driver.
// Inputs:
// - url_prefix: "http://127.0.0.1:8080"
// - version: "auto", "h1", "h2", "h3"
// - skip_verify: true/false (for self-signed upstreams)
// - websocket: true/false (enable websocket upgrade tunneling)
// Branches:
// - onSuccess: Executed when upstream returns a response (usually NewSendResponse).
// - onFailure: Executed when upstream fails (usually NewAbortConnection).
func NewFetchUpstream(urlPrefix string, version string, skipVerify bool, websocket bool, onSuccess ProcessingStep, onFailure ProcessingStep) ProcessingStep {
	return ProcessingStep{
		"internal.driver.upstream": PluginInstance{
			Input: map[string]interface{}{
				"url_prefix":  urlPrefix,
				"version":     version,
				"skip_verify": skipVerify,
				"websocket":   websocket,
			},
			Output: map[string]ProcessingStep{
				"success": onSuccess,
				"failure": onFailure,
			},
		},
	}
}

// NewCgiExecution creates a ProcessingStep for internal.driver.cgi.
// command: absolute path to the executable.
// script: optional script path (e.g. for php-cgi).
func NewCgiExecution(command string, script string, onSuccess ProcessingStep, onFailure ProcessingStep) ProcessingStep {
	inputs := map[string]interface{}{
		"command": command,
		"timeout": 5,
		// Standard CGI Mapping
		"method": "{{req.method}}",
		"uri":    "{{req.path}}",
		// Explicitly map query from KV
		"query": "{{req.query}}",
	}
	if script != "" {
		inputs["script"] = script
	}

	return ProcessingStep{
		"internal.driver.cgi": PluginInstance{
			Input: inputs,
			Output: map[string]ProcessingStep{
				"success": onSuccess,
				"failure": onFailure,
			},
		},
	}
}

// NewSendResponse creates a terminator that flushes the current Container response to the client.
// Plugin: internal.terminator.response
// Use this after FetchUpstream to actually send the data back.
func NewSendResponse() ProcessingStep {
	return ProcessingStep{
		"internal.terminator.response": PluginInstance{
			Input:  map[string]interface{}{}, // No inputs needed, consumes Container
			Output: nil,                      // Terminators do not have output branches
		},
	}
}

// NewL7StaticResponse creates a simple fixed response.
// Plugin: internal.terminator.response
// Useful for mocking or error pages without upstream.
func NewL7StaticResponse(status int, body string) ProcessingStep {
	return ProcessingStep{
		"internal.terminator.response": PluginInstance{
			Input: map[string]interface{}{
				"status": status,
				"body":   body,
			},
			Output: nil,
		},
	}
}
