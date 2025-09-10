/* src/routing.rs */

use crate::{
    error::VaneError,
    models::Route,
    path_matcher::{MatchScore, get_match_score},
    state::AppState,
};
use std::sync::Arc;

/// Finds the best target URL based on wildcard matching and specificity.
/// Returns an error if the configuration is ambiguous.
pub fn find_target_url(
    host: &str,
    path: &str,
    state: &Arc<AppState>,
) -> Result<Option<String>, VaneError> {
    let domain_config = match state.config.domains.get(host) {
        Some(cfg) => cfg,
        None => return Ok(None),
    };

    let mut best_matches: Vec<(&Route, MatchScore)> = Vec::new();

    for route in &domain_config.routes {
        if let Some(score) = get_match_score(&route.path, path) {
            if best_matches.is_empty() {
                // First match found.
                best_matches.push((route, score));
            } else {
                let best_score = &best_matches[0].1;
                if &score > best_score {
                    // This match is better than all previous ones.
                    best_matches.clear();
                    best_matches.push((route, score));
                } else if score == *best_score {
                    // Another match with the same best score.
                    best_matches.push((route, score));
                }
            }
        }
    }

    if best_matches.len() > 1 {
        // Multiple routes matched with the same highest priority. This is an error.
        Err(VaneError::AmbiguousRoute)
    } else if let Some((best_route, _)) = best_matches.first() {
        // Exactly one best match was found.
        Ok(best_route.targets.first().cloned())
    } else {
        // No match was found.
        Ok(None)
    }
}
