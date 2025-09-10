/* src/ratelimit.rs */

use crate::state::ConfigurableRateLimiter;
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::sync::Arc;

/// Represents the quality of a match between a path and a pattern.
/// A higher score indicates a better, more specific match.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MatchScore {
    /// Number of exact (non-wildcard) path segments. Higher is better.
    exact_parts: usize,
    /// Total number of segments in the pattern. Longer is generally more specific.
    total_parts: usize,
}

/// A found match, containing the limiter to apply and its calculated score.
// FIX: Make fields public so the middleware module can access them.
pub struct FoundMatch<'a> {
    pub limiter: &'a Arc<ConfigurableRateLimiter>,
    pub pattern: &'a str,
    score: MatchScore,
}

/// Calculates a specificity score for a pattern matching a given path.
fn get_match_score(pattern: &str, path: &str) -> Option<MatchScore> {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if pattern_parts.len() > path_parts.len() {
        return None;
    }

    let mut exact_parts = 0;

    for (i, p_part) in pattern_parts.iter().enumerate() {
        if *p_part == "*" {
            continue;
        }
        if Some(p_part) != path_parts.get(i) {
            return None;
        }
        exact_parts += 1;
    }

    Some(MatchScore {
        exact_parts,
        total_parts: pattern_parts.len(),
    })
}

/// Finds the single best matching rate limiter from a map of pattern-limiter pairs.
/// "Best" is defined as the most specific match (highest `MatchScore`).
pub fn find_best_match<'a>(
    limiters: &'a HashMap<String, Arc<ConfigurableRateLimiter>>,
    full_path: &str,
) -> Option<FoundMatch<'a>> {
    let mut best_match: Option<FoundMatch<'a>> = None;

    for (pattern, limiter) in limiters.iter() {
        if let Some(score) = get_match_score(pattern, full_path) {
            log(
                LogLevel::Debug,
                &format!(
                    "Path '{}' matched pattern '{}' with score {:?}",
                    full_path, pattern, score
                ),
            );

            let current_match = FoundMatch {
                limiter,
                score,
                pattern,
            };

            match best_match.as_mut() {
                None => {
                    // This is the first valid match.
                    best_match = Some(current_match);
                }
                Some(best) => {
                    // FIX: Simplified logic. A new match is better if its score is higher.
                    // The tie-breaking logic was incorrect and has been removed.
                    // Ord trait on MatchScore handles the prioritization correctly.
                    if current_match.score > best.score {
                        *best = current_match;
                    }
                }
            }
        }
    }

    if let Some(ref found) = best_match {
        log(
            LogLevel::Debug,
            &format!("Final best match is pattern '{}'", found.pattern),
        );
    }

    best_match
}
