/* integration/pkg/config/basic/legacy.go */
package basic

// Target represents a backend destination.
type Target struct {
	Ip   string `yaml:"ip"`
	Port int    `yaml:"port"`
}

// Strategy defines how targets are selected (random, serial, fastest).
type Strategy string

const (
	StrategyRandom  Strategy = "random"
	StrategySerial  Strategy = "serial"
	StrategyFastest Strategy = "fastest"
)

// DetectMethod defines how Vane identifies the protocol.
type DetectMethod string

const (
	DetectMagic    DetectMethod = "magic"
	DetectPrefix   DetectMethod = "prefix"
	DetectRegex    DetectMethod = "regex"
	DetectFallback DetectMethod = "fallback"
)

// Detect holds protocol detection rules.
type Detect struct {
	Method  DetectMethod `yaml:"method"`
	Pattern string       `yaml:"pattern"`
}

// Forward defines load balancing rules.
type Forward struct {
	Strategy  Strategy `yaml:"strategy"`
	Targets   []Target `yaml:"targets"`
	Fallbacks []Target `yaml:"fallbacks,omitempty"`
}

// TcpDestination defines where TCP traffic goes.
type TcpDestination struct {
	Type    string   `yaml:"type"` // "forward" or "resolver"
	Forward *Forward `yaml:"forward,omitempty"`
}

// TcpProtocolRule corresponds to a single rule in protocols[].
type TcpProtocolRule struct {
	Name        string         `yaml:"name"`
	Priority    int            `yaml:"priority"`
	Detect      Detect         `yaml:"detect"`
	Destination TcpDestination `yaml:"destination"`
}

// LegacyTcpConfig matches the [protocols] structure for TCP.
type LegacyTcpConfig struct {
	Protocols []TcpProtocolRule `yaml:"protocols"`
}

// UdpDestination defines where UDP traffic goes.
type UdpDestination struct {
	Type    string   `yaml:"type"`
	Forward *Forward `yaml:"forward,omitempty"`
}

// UdpProtocolRule corresponds to a single rule in protocols[].
type UdpProtocolRule struct {
	Name        string         `yaml:"name"`
	Priority    int            `yaml:"priority"`
	Detect      Detect         `yaml:"detect"`
	Destination UdpDestination `yaml:"destination"`
}

// LegacyUdpConfig matches the [protocols] structure for UDP.
type LegacyUdpConfig struct {
	Protocols []UdpProtocolRule `yaml:"protocols"`
}
