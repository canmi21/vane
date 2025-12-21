/* integration/pkg/config/advanced/terminator.go */
package advanced

// NewTransparentProxy creates a ProcessingStep that terminates the flow
// by proxying to a specific IP:Port.
// Plugin: internal.transport.proxy
// Params: target.ip (string), target.port (int)
func NewTransparentProxy(ip string, port int) ProcessingStep {
	return ProcessingStep{
		// FIXED: Plugin name matches the Rust implementation
		"internal.transport.proxy": PluginInstance{
			Input: map[string]interface{}{
				// FIXED: Split target into ip and port as per ParamDef
				"target.ip":   ip,
				"target.port": port,
			},
			// Terminators usually have no output
			Output: nil,
		},
	}
}
