/* integration/pkg/config/advanced/terminator.go */
package advanced

// NewTransparentProxy creates a ProcessingStep that terminates the flow
// by proxying to a specific IP:Port.
// Plugin: internal.transport.proxy
// Params: target.ip (string), target.port (int)
func NewTransparentProxy(ip string, port int) ProcessingStep {
	return ProcessingStep{
		"internal.transport.proxy": PluginInstance{
			Input: map[string]interface{}{
				"target.ip":   ip,
				"target.port": port,
			},
			Output: nil,
		},
	}
}

// NewAbortConnection creates a ProcessingStep that immediately closes the connection.
// Plugin: internal.transport.abort
func NewAbortConnection() ProcessingStep {
	return ProcessingStep{
		"internal.transport.abort": PluginInstance{
			Input:  map[string]interface{}{},
			Output: nil,
		},
	}
}

// NewUpgrade creates a ProcessingStep that upgrades the connection to L4+ or L7.
// Plugin: internal.transport.upgrade
// protocol: "tls", "http", "quic"
func NewUpgrade(protocol string) ProcessingStep {
	return ProcessingStep{
		"internal.transport.upgrade": PluginInstance{
			Input: map[string]interface{}{
				"protocol": protocol,
			},
			Output: nil,
		},
	}
}
