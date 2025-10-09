/* src/path_matcher.rs */

// MODIFIED: Renamed from `Match` to `MatchScore` and made public for ratelimit.rs.
// This enum ranks how well a path matches a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchScore {
    // e.g., request "/api/v1" matches route "/"
    Root,
    // e.g., request "/api/v1" matches route "/api/*"
    Wildcard,
    // e.g., request "/about" matches route "/about"
    Exact,
}

// MODIFIED: Renamed from `matches` to `get_match_score` for clarity and use in ratelimit.rs.
/// Checks if a request path matches a route's path pattern.
/// Returns a `MatchScore` enum indicating the quality of the match if successful.
pub fn get_match_score(request_path: &str, route_path: &str) -> Option<MatchScore> {
    if let Some(wildcard_base) = route_path.strip_suffix("/*") {
        // Handle wildcard match (e.g., /api/*)
        // This should match the base path exactly or be a sub-path.
        if request_path.starts_with(wildcard_base)
            && (request_path.len() == wildcard_base.len()
                || request_path.as_bytes().get(wildcard_base.len()) == Some(&b'/'))
        {
            // The root wildcard "/" should have lower priority than more specific wildcards.
            if wildcard_base == "/" {
                return Some(MatchScore::Root);
            }
            return Some(MatchScore::Wildcard);
        }
    } else if request_path == route_path {
        // Handle exact match. The root path "/" is a special case of exact match.
        if request_path == "/" {
            return Some(MatchScore::Root);
        }
        return Some(MatchScore::Exact);
    }

    None
}
