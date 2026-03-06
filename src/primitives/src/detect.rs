use serde::{Deserialize, Serialize};

/// Method used for L4 protocol detection.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectMethod {
    Magic,
    Prefix,
    Regex,
    Fallback,
}

/// A protocol detection rule: method + pattern to match against.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Detect {
    pub method: DetectMethod,
    pub pattern: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn detect_method_serde_roundtrip() {
        for method in [
            DetectMethod::Magic,
            DetectMethod::Prefix,
            DetectMethod::Regex,
            DetectMethod::Fallback,
        ] {
            let json = serde_json::to_string(&method).unwrap();
            let back: DetectMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(method, back);
        }
    }

    #[test]
    fn detect_method_snake_case() {
        assert_eq!(serde_json::to_string(&DetectMethod::Magic).unwrap(), r#""magic""#);
        assert_eq!(serde_json::to_string(&DetectMethod::Prefix).unwrap(), r#""prefix""#);
        assert_eq!(serde_json::to_string(&DetectMethod::Regex).unwrap(), r#""regex""#);
        assert_eq!(serde_json::to_string(&DetectMethod::Fallback).unwrap(), r#""fallback""#);
    }

    #[test]
    fn detect_struct_serde_roundtrip() {
        let detect = Detect {
            method: DetectMethod::Regex,
            pattern: r"^GET\s".to_owned(),
        };
        let json = serde_json::to_string(&detect).unwrap();
        let back: Detect = serde_json::from_str(&json).unwrap();
        assert_eq!(detect, back);
    }
}
