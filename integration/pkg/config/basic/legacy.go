/* integration/pkg/config/basic/legacy.go */
package basic

// --- Common Types (model.rs) ---

type Target struct {
	Ip     string `yaml:"ip,omitempty"`
	Domain string `yaml:"domain,omitempty"`
	Node   string `yaml:"node,omitempty"`
	Port   uint16 `yaml:"port"`
}

type DetectMethod string

const (
	DetectMagic    DetectMethod = "magic"
	DetectPrefix   DetectMethod = "prefix"
	DetectRegex    DetectMethod = "regex"
	DetectFallback DetectMethod = "fallback"
)

type Detect struct {
	Method  DetectMethod `yaml:"method"`
	Pattern string       `yaml:"pattern"`
}

type Strategy string

const (
	StrategyRandom  Strategy = "random"
	StrategySerial  Strategy = "serial"
	StrategyFastest Strategy = "fastest"
)

type Forward struct {
	Strategy  Strategy `yaml:"strategy"`
	Targets   []Target `yaml:"targets"`
	Fallbacks []Target `yaml:"fallbacks"`
}

// --- TCP Config (tcp.rs) ---

type TcpSession struct {
	Keepalive bool   `yaml:"keepalive"`
	Timeout   uint64 `yaml:"timeout"`
}

// TcpDestination uses a custom structure because Rust uses a tagged enum:
// #[serde(tag = "type", rename_all = "snake_case")]
// enum TcpDestination { Resolver { resolver: String }, Forward { forward: Forward } }
type TcpDestination struct {
	Type     string   `yaml:"type"`
	Resolver string   `yaml:"resolver,omitempty"`
	Forward  *Forward `yaml:"forward,omitempty"`
}

type TcpProtocolRule struct {
	Name        string         `yaml:"name"`
	Priority    uint32         `yaml:"priority"`
	Detect      Detect         `yaml:"detect"`
	Session     *TcpSession    `yaml:"session,omitempty"`
	Destination TcpDestination `yaml:"destination"`
}

type LegacyTcpConfig struct {
	Protocols []TcpProtocolRule `yaml:"protocols"`
}

// --- UDP Config (udp.rs) ---

// UdpDestination is similar to TcpDestination
type UdpDestination struct {
	Type     string   `yaml:"type"`
	Resolver string   `yaml:"resolver,omitempty"`
	Forward  *Forward `yaml:"forward,omitempty"`
}

type UdpProtocolRule struct {
	Name        string         `yaml:"name"`
	Priority    uint32         `yaml:"priority"`
	Detect      Detect         `yaml:"detect"`
	Destination UdpDestination `yaml:"destination"`
}

type LegacyUdpConfig struct {
	Protocols []UdpProtocolRule `yaml:"protocols"`
}
