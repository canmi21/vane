/* test/integration/pkg/config/advanced/middleware.go */

package advanced

// NewMiddlewareStep creates a generic middleware step.
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
func NewProtocolDetect(method string, onTrue ProcessingStep, onFalse ProcessingStep) ProcessingStep {
	return NewMiddlewareStep(
		"internal.protocol.detect",
		map[string]interface{}{
			"method":  method,
			"payload": "{{req.peek_buffer_hex}}",
		},
		map[string]ProcessingStep{
			"true":  onTrue,
			"false": onFalse,
		},
	)
}

// NewRateLimitSec creates a per-second rate limit step.
func NewRateLimitSec(key string, limit int, onTrue ProcessingStep, onFalse ProcessingStep) ProcessingStep {
	return NewMiddlewareStep(
		"internal.common.ratelimit.sec",
		map[string]interface{}{
			"key":   key,
			"limit": limit,
		},
		map[string]ProcessingStep{
			"true":  onTrue,
			"false": onFalse,
		},
	)
}

// NewMatch creates a binary matching step (If Left == Right).
// Plugin: internal.common.match
// left: Value A (e.g. "{{tls.sni}}")
// right: Value B (e.g. "example.com")
// onTrue: Executed if match succeeds.
// onFalse: Executed if match fails.
func NewMatch(left string, right string, onTrue ProcessingStep, onFalse ProcessingStep) ProcessingStep {
	return NewMiddlewareStep(
		"internal.common.match",
		map[string]interface{}{
			"left":     left,
			"right":    right,
			"operator": "==",
		},
		map[string]ProcessingStep{
			"true":  onTrue,
			"false": onFalse,
		},
	)
}
