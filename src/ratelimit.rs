/* src/ratelimit.rs */

// FIX: Import shared matching logic from the new module.
use crate::{path_matcher, state::ConfigurableRateLimiter};
use fancy_log::{LogLevel, log};
use std::collections::HashMap;
use std::sync::Arc;

// FIX: This struct is now public and lives in the new module.
// We just need to refer to it with its full path.
pub struct FoundMatch<'a> {
    pub limiter: &'a Arc<ConfigurableRateLimiter>,
    pub pattern: &'a str,
    score: path_matcher::MatchScore,
}

/// Finds the single best matching rate limiter from a map of pattern-limiter pairs.
pub fn find_best_match<'a>(
    limiters: &'a HashMap<String, Arc<ConfigurableRateLimiter>>,
    full_path: &str,
) -> Option<FoundMatch<'a>> {
    let mut best_match: Option<FoundMatch<'a>> = None;

    for (pattern, limiter) in limiters.iter() {
        // FIX: Call the shared function.
        if let Some(score) = path_matcher::get_match_score(pattern, full_path) {
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
                None => best_match = Some(current_match),
                Some(best) => {
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
