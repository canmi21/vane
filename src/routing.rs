/* src/routing.rs */

use crate::{
    error::VaneError,
    models::Route,
    path_matcher::{self, Match},
    state::AppState,
};
use std::sync::Arc;

/// Finds the best matching route for a given host and path.
/// The best match is the most specific one (Exact > Wildcard).
/// Returns a reference to the matched `Route` configuration.
pub fn find_matched_route<'a>(
    host: &str,
    path: &str,
    state: &'a Arc<AppState>,
) -> Result<Option<&'a Route>, VaneError> {
    let domain_config = match state.config.domains.get(host) {
        Some(cfg) => cfg,
        None => return Err(VaneError::HostNotFound),
    };

    let mut best_match: Option<(Match, &Route)> = None;

    for route_rule in &domain_config.routes {
        if let Some(current_match) = path_matcher::matches(path, &route_rule.path) {
            if let Some((best_match_type, _)) = best_match {
                // If the new match is more specific, it becomes the new best match.
                if current_match > best_match_type {
                    best_match = Some((current_match, route_rule));
                }
                // If they are equally specific, the configuration is ambiguous.
                else if current_match == best_match_type {
                    return Err(VaneError::AmbiguousRoute);
                }
            } else {
                // This is the first match we've found.
                best_match = Some((current_match, route_rule));
            }
        }
    }

    // Return the matched `Route` struct itself.
    Ok(best_match.map(|(_, route)| route))
}
