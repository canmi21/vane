/* engine/src/proxy/domain/hotswap.rs */

use crate::modules::domain::entrance as domain_helper;
use arc_swap::{ArcSwap, Guard};
use fancy_log::{LogLevel, log};
use once_cell::sync::Lazy;
use std::sync::Arc;

// The global, thread-safe, atomically swappable list of all known domains.
static DOMAIN_LIST: Lazy<ArcSwap<Vec<String>>> = Lazy::new(|| ArcSwap::from(Arc::new(Vec::new())));

/// Retrieves a read-only guard to the current list of domains.
pub fn get_domain_list() -> Guard<Arc<Vec<String>>> {
	DOMAIN_LIST.load()
}

/// Scans the configuration directory for domain folders and atomically updates the in-memory list.
pub async fn reload_domain_list() {
	log(LogLevel::Debug, "Reloading domain list from filesystem...");
	let mut domains = domain_helper::list_domains_internal().await;
	domains.sort();

	let old_list = DOMAIN_LIST.load();
	if **old_list != domains {
		log(
			LogLevel::Info,
			&format!("Domain list updated. New list: {:?}", &domains),
		);
		DOMAIN_LIST.store(Arc::new(domains));
	} else {
		log(LogLevel::Debug, "Domain list has not changed.");
	}
}
