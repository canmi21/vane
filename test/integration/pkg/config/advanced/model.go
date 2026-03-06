/* test/integration/pkg/config/advanced/model.go */

package advanced

// ProcessingStep represents a step in the flow.
// It maps a Plugin Name to a Plugin Instance.
// In JSON: { "plugin.name": { "input": {...}, "output": {...} } }
type ProcessingStep map[string]PluginInstance

type PluginInstance struct {
	Input  map[string]interface{}    `json:"input"`
	Output map[string]ProcessingStep `json:"output,omitempty"`
}

// L4FlowConfig is the top-level structure for flow-based L4 config.
// Corresponds to TcpConfig::Flow or UdpConfig::Flow in Rust.
type L4FlowConfig struct {
	Connection ProcessingStep `json:"connection"`
}

// ApplicationConfig corresponds to Rust's ApplicationConfig struct.
// Location: application/httpx.json (or other protocol names).
type ApplicationConfig struct {
	Pipeline ProcessingStep `json:"pipeline"`
}
