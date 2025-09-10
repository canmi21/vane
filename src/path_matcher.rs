/* src/path_matcher.rs */

/// Represents the quality of a match between a path and a pattern.
/// A higher score indicates a better, more specific match.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchScore {
    /// Number of exact (non-wildcard) path segments. Higher is better.
    pub exact_parts: usize,
    /// Total number of segments in the pattern. Longer is generally more specific.
    pub total_parts: usize,
}

/// Calculates a specificity score for a pattern matching a given path.
///
/// - Returns `Some(MatchScore)` if the pattern matches the path prefix.
/// - Returns `None` if the pattern does not match.
///
/// The matching logic supports `*` as a single-segment wildcard.
pub fn get_match_score(pattern: &str, path: &str) -> Option<MatchScore> {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // The pattern cannot be longer than the path for a prefix match.
    if pattern_parts.len() > path_parts.len() {
        return None;
    }

    let mut exact_parts = 0;

    for (i, p_part) in pattern_parts.iter().enumerate() {
        if *p_part == "*" {
            continue; // Wildcard matches any segment.
        }
        if Some(p_part) != path_parts.get(i) {
            return None; // Mismatch on an exact part.
        }
        exact_parts += 1;
    }

    Some(MatchScore {
        exact_parts,
        total_parts: pattern_parts.len(),
    })
}
