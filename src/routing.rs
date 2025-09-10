/* src/routing.rs */

use crate::models::Route;
use crate::state::AppState;
use std::sync::Arc;

pub fn find_target_url(host: &str, path: &str, state: &Arc<AppState>) -> Option<String> {
    let domain_config = state.config.domains.get(host)?;

    let route: &Route = domain_config
        .routes
        .iter()
        .find(|r| path.starts_with(&r.path))?;

    route.targets.first().cloned()
}
