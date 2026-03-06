use crate::flow::context::ExecutionContext;
use crate::flow::plugin::{BranchAction, Middleware};

/// How a detection rule matches against peeked bytes.
#[derive(Debug, Clone)]
pub enum MatchCondition {
    /// Match exact bytes at a given offset.
    MagicBytes { offset: usize, bytes: Vec<u8> },
    /// Match a UTF-8 prefix (case-sensitive).
    StringPrefix(String),
    /// Always matches — use as a catch-all at the end of the rule list.
    Fallback,
}

impl MatchCondition {
    fn matches(&self, data: &[u8]) -> bool {
        match self {
            Self::MagicBytes { offset, bytes } => {
                data.len() >= offset + bytes.len() && data[*offset..].starts_with(bytes)
            }
            Self::StringPrefix(prefix) => data.starts_with(prefix.as_bytes()),
            Self::Fallback => true,
        }
    }
}

/// A single protocol detection rule: condition + protocol label.
#[derive(Debug, Clone)]
pub struct DetectRule {
    pub protocol: String,
    pub condition: MatchCondition,
}

/// Middleware that inspects peeked bytes and returns the detected protocol as the branch name.
///
/// Rules are evaluated sequentially; first match wins.
/// If no rule matches, the middleware returns an error.
pub struct ProtocolDetect {
    rules: Vec<DetectRule>,
}

impl ProtocolDetect {
    pub const fn new(rules: Vec<DetectRule>) -> Self {
        Self { rules }
    }

    /// Create with a sensible set of default rules (TLS, HTTP, fallback "unknown").
    pub fn with_defaults() -> Self {
        Self {
            rules: default_rules(),
        }
    }
}

impl Middleware for ProtocolDetect {
    fn execute(
        &self,
        _params: &serde_json::Value,
        ctx: &dyn ExecutionContext,
    ) -> Result<BranchAction, anyhow::Error> {
        let data = ctx
            .peek_data()
            .ok_or_else(|| anyhow::anyhow!("no peek data for protocol detection"))?;

        for rule in &self.rules {
            if rule.condition.matches(data) {
                return Ok(BranchAction {
                    branch: rule.protocol.clone(),
                    updates: vec![],
                });
            }
        }

        Err(anyhow::anyhow!("no detection rule matched"))
    }
}

/// Default detection rules: TLS (4 content types), HTTP (all methods), fallback.
pub fn default_rules() -> Vec<DetectRule> {
    let mut rules = Vec::new();

    // TLS record types: content type byte followed by major version 0x03
    for content_type in [0x14, 0x15, 0x16, 0x17] {
        rules.push(DetectRule {
            protocol: "tls".to_owned(),
            condition: MatchCondition::MagicBytes {
                offset: 0,
                bytes: vec![content_type, 0x03],
            },
        });
    }

    // HTTP methods
    for method in [
        "GET ", "POST ", "PUT ", "DELETE ", "HEAD ", "OPTIONS ", "CONNECT ", "PATCH ", "PRI * ",
    ] {
        rules.push(DetectRule {
            protocol: "http".to_owned(),
            condition: MatchCondition::StringPrefix(method.to_owned()),
        });
    }

    // Catch-all
    rules.push(DetectRule {
        protocol: "unknown".to_owned(),
        condition: MatchCondition::Fallback,
    });

    rules
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use vane_primitives::kv::KvStore;

    /// Mock context with configurable peek data.
    struct MockContext {
        peer: SocketAddr,
        server: SocketAddr,
        kv: KvStore,
        peek: Option<Vec<u8>>,
    }

    impl MockContext {
        fn with_peek(data: &[u8]) -> Self {
            let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
            let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
            let kv = KvStore::new(&peer, &server, "tcp");
            Self {
                peer,
                server,
                kv,
                peek: Some(data.to_vec()),
            }
        }

        fn without_peek() -> Self {
            let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234);
            let server = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
            let kv = KvStore::new(&peer, &server, "tcp");
            Self {
                peer,
                server,
                kv,
                peek: None,
            }
        }
    }

    impl ExecutionContext for MockContext {
        fn peer_addr(&self) -> SocketAddr {
            self.peer
        }
        fn server_addr(&self) -> SocketAddr {
            self.server
        }
        fn kv(&self) -> &KvStore {
            &self.kv
        }
        fn kv_mut(&mut self) -> &mut KvStore {
            &mut self.kv
        }
        fn take_stream(&mut self) -> Option<tokio::net::TcpStream> {
            None
        }
        fn peek_data(&self) -> Option<&[u8]> {
            self.peek.as_deref()
        }
    }

    fn detect_with_defaults(data: &[u8]) -> Result<String, anyhow::Error> {
        let pd = ProtocolDetect::with_defaults();
        let ctx = MockContext::with_peek(data);
        pd.execute(&serde_json::Value::Null, &ctx)
            .map(|a| a.branch)
    }

    // -- TLS detection --

    #[test]
    fn tls_client_hello() {
        // 0x16 = Handshake, 0x03 0x01 = TLS 1.0
        let data = [0x16, 0x03, 0x01, 0x00, 0x05, 0x01, 0x00, 0x00];
        assert_eq!(detect_with_defaults(&data).unwrap(), "tls");
    }

    #[test]
    fn tls_change_cipher_spec() {
        let data = [0x14, 0x03, 0x03, 0x00, 0x01];
        assert_eq!(detect_with_defaults(&data).unwrap(), "tls");
    }

    #[test]
    fn tls_alert() {
        let data = [0x15, 0x03, 0x03, 0x00, 0x02];
        assert_eq!(detect_with_defaults(&data).unwrap(), "tls");
    }

    #[test]
    fn tls_application_data() {
        let data = [0x17, 0x03, 0x03, 0x00, 0x10];
        assert_eq!(detect_with_defaults(&data).unwrap(), "tls");
    }

    // -- HTTP detection --

    #[test]
    fn http_get() {
        assert_eq!(
            detect_with_defaults(b"GET / HTTP/1.1\r\n").unwrap(),
            "http"
        );
    }

    #[test]
    fn http_post() {
        assert_eq!(
            detect_with_defaults(b"POST /api HTTP/1.1\r\n").unwrap(),
            "http"
        );
    }

    #[test]
    fn http2_preface() {
        assert_eq!(
            detect_with_defaults(b"PRI * HTTP/2.0\r\n").unwrap(),
            "http"
        );
    }

    // -- Fallback / unknown --

    #[test]
    fn random_bytes_with_defaults_returns_unknown() {
        assert_eq!(detect_with_defaults(&[0xDE, 0xAD]).unwrap(), "unknown");
    }

    #[test]
    fn no_fallback_rule_and_random_bytes_returns_error() {
        let rules = vec![DetectRule {
            protocol: "tls".to_owned(),
            condition: MatchCondition::MagicBytes {
                offset: 0,
                bytes: vec![0x16, 0x03],
            },
        }];
        let pd = ProtocolDetect::new(rules);
        let ctx = MockContext::with_peek(&[0xDE, 0xAD]);
        assert!(pd.execute(&serde_json::Value::Null, &ctx).is_err());
    }

    // -- First match wins --

    #[test]
    fn first_match_wins() {
        let rules = vec![
            DetectRule {
                protocol: "first".to_owned(),
                condition: MatchCondition::Fallback,
            },
            DetectRule {
                protocol: "second".to_owned(),
                condition: MatchCondition::Fallback,
            },
        ];
        let pd = ProtocolDetect::new(rules);
        let ctx = MockContext::with_peek(&[0x00]);
        assert_eq!(
            pd.execute(&serde_json::Value::Null, &ctx)
                .unwrap()
                .branch,
            "first"
        );
    }

    // -- No peek data --

    #[test]
    fn no_peek_data_returns_error() {
        let pd = ProtocolDetect::with_defaults();
        let ctx = MockContext::without_peek();
        assert!(pd.execute(&serde_json::Value::Null, &ctx).is_err());
    }

    // -- Empty peek data + Fallback --

    #[test]
    fn empty_peek_with_fallback_returns_unknown() {
        assert_eq!(detect_with_defaults(&[]).unwrap(), "unknown");
    }

    // -- MagicBytes with offset --

    #[test]
    fn magic_bytes_with_offset() {
        let rules = vec![DetectRule {
            protocol: "custom".to_owned(),
            condition: MatchCondition::MagicBytes {
                offset: 2,
                bytes: vec![0xAB, 0xCD],
            },
        }];
        let pd = ProtocolDetect::new(rules);
        let ctx = MockContext::with_peek(&[0x00, 0x00, 0xAB, 0xCD, 0xEF]);
        assert_eq!(
            pd.execute(&serde_json::Value::Null, &ctx)
                .unwrap()
                .branch,
            "custom"
        );
    }

    // -- Edge cases for MatchCondition::matches --

    #[test]
    fn data_shorter_than_pattern() {
        let cond = MatchCondition::MagicBytes {
            offset: 0,
            bytes: vec![0x16, 0x03, 0x01],
        };
        // Only 2 bytes, pattern needs 3
        assert!(!cond.matches(&[0x16, 0x03]));
    }

    #[test]
    fn exact_length_match() {
        let cond = MatchCondition::MagicBytes {
            offset: 0,
            bytes: vec![0x16, 0x03],
        };
        assert!(cond.matches(&[0x16, 0x03]));
    }

    #[test]
    fn offset_beyond_data_length() {
        let cond = MatchCondition::MagicBytes {
            offset: 10,
            bytes: vec![0x01],
        };
        assert!(!cond.matches(&[0x01, 0x02]));
    }

    #[test]
    fn string_prefix_partial_match() {
        let cond = MatchCondition::StringPrefix("GET ".to_owned());
        assert!(!cond.matches(b"GE"));
    }
}
