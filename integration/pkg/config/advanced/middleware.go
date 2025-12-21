/* integration/pkg/config/advanced/middleware.go */
package advanced

// NewMiddlewareStep creates a generic middleware step.
// outputBranches maps branch names ("true", "false", etc.) to the next step.
func NewMiddlewareStep(
	pluginName string,
	inputs map[string]interface{},
	outputBranches map[string]ProcessingStep,
) ProcessingStep {
	return ProcessingStep{
		pluginName: PluginInstance{
			Input:  inputs,
			Output: outputBranches,
		},
	}
}

// NewProtocolDetect creates a detection step.
// method: "http", "tls", "dns", "quic"
// onTrue: Step to execute if detection matches.
// onFalse: Step to execute if detection fails.
func NewProtocolDetect(method string, onTrue ProcessingStep, onFalse ProcessingStep) ProcessingStep {
	return NewMiddlewareStep(
		"internal.protocol.detect",
		map[string]interface{}{
			"method": method,
			// FIXED: Correct L4 context variable is req.peek_buffer_hex
			// The plugin expects a hex string, and this variable provides exactly that.
			"payload": "{{req.peek_buffer_hex}}",
		},
		map[string]ProcessingStep{
			"true":  onTrue,
			"false": onFalse,
		},
	)
}
