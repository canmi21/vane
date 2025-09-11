/* src/routing.rs */

use crate::{
    error::VaneError,
    // MODIFIED: Import the correct items from models and path_matcher.
    models::{DomainConfig, Route},
    path_matcher::{self, MatchScore},
    state::AppState,
};
use anyhow::Result;
use std::sync::Arc;

/// Finds the best-matching route and returns its list of target URLs.
/// The targets are returned in their configured order for failover attempts.
pub fn find_target_urls(
    host: &str,
    path: &str,
    state: &Arc<AppState>,
) -> Result<Option<Vec<String>>, VaneError> {
    // Find the domain configuration for the given host.
    let domain_config = state
        .config
        .domains
        .get(host)
        .ok_or(VaneError::HostNotFound)?;

    find_best_route(path, domain_config)
}

/// Iterates through routes to find the best match based on path specificity.
fn find_best_route(
    path: &str,
    domain_config: &DomainConfig,
) -> Result<Option<Vec<String>>, VaneError> {
    // MODIFIED: This logic now correctly uses get_match_score and MatchScore.
    let mut best_match: Option<(MatchScore, &Route)> = None;
    let mut ambiguous = false;

    // Iterate over all configured routes for the domain.
    for route in &domain_config.routes {
        // Get a score for the current route against the request path.
        if let Some(current_score) = path_matcher::get_match_score(&route.path, path) {
            match &mut best_match {
                Some((best_score, _)) => {
                    // A new match is better if its score is higher.
                    if current_score > *best_score {
                        *best_score = current_score;
                        best_match = Some((best_score.clone(), route));
                        ambiguous = false;
                    } else if current_score == *best_score {
                        // If scores are equal, the configuration is ambiguous.
                        ambiguous = true;
                    }
                }
                None => {
                    // This is the first match found.
                    best_match = Some((current_score, route));
                }
            }
        }
    }

    if ambiguous {
        // If two routes have the same best score, it's an error.
        return Err(VaneError::AmbiguousRoute);
    }

    // If a best match was found, clone and return its list of target URLs.
    Ok(best_match.map(|(_, route)| route.targets.clone()))
}
