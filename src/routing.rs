/* src/routing.rs */

use crate::models::Route;
use crate::state::AppState;
use std::sync::Arc;

pub fn find_target_url(host: &str, path: &str, state: &Arc<AppState>) -> Option<String> {
    // Find domain config matching the host
    let domain_config = state.config.domains.get(host)?;

    // Find the first route that matches the request path prefix
    let route: &Route = domain_config
        .routes
        .iter()
        .find(|r| path.starts_with(&r.path))?;

    // Return the primary target URL
    route.targets.first().cloned()
}
