/* engine/src/config/template.rs */

use crate::daemon::config;
use fancy_log::{LogLevel, log};
use rust_embed::RustEmbed;
use std::fs;

#[derive(RustEmbed)]
#[folder = "./../templates/"]
struct Asset;

pub fn initialize_templates() {
	let config_dir = config::get_config_dir();
	let templates_path = config_dir.join("templates");

	if !templates_path.exists() {
		if let Err(e) = fs::create_dir_all(&templates_path) {
			log(
				LogLevel::Error,
				&format!(
					"! Failed to create templates dir {}: {}",
					templates_path.display(),
					e
				),
			);
			return;
		}
		log(
			LogLevel::Info,
			&format!("+ Created \"{}\"", templates_path.display()),
		);
	}

	let mut is_empty = true;
	match fs::read_dir(&templates_path) {
		Ok(mut rd) => {
			if rd.next().is_some() {
				is_empty = false;
			}
		}
		Err(e) => {
			log(
				LogLevel::Error,
				&format!(
					"! Failed to read templates dir {}: {}",
					templates_path.display(),
					e
				),
			);
			return;
		}
	}

	if !is_empty {
		log(
			LogLevel::Debug,
			&format!(
				"Templates directory {} not empty, skipping extraction.",
				templates_path.display()
			),
		);
		return;
	}

	log(
		LogLevel::Info,
		&format!(
			"Templates directory is empty. Extracting embedded templates to {}",
			templates_path.display()
		),
	);

	for filename in Asset::iter() {
		if let Some(asset) = Asset::get(&filename) {
			let target_path = templates_path.join(filename.as_ref());
			if let Some(parent) = target_path.parent() {
				if let Err(e) = fs::create_dir_all(parent) {
					log(
						LogLevel::Error,
						&format!(
							"! Failed to create parent dir for {}: {}",
							target_path.display(),
							e
						),
					);
					continue;
				}
			}

			if let Err(e) = fs::write(&target_path, asset.data.as_ref()) {
				log(
					LogLevel::Error,
					&format!(
						"! Failed to write template file '{}': {}",
						target_path.display(),
						e
					),
				);
			} else {
				log(
					LogLevel::Debug,
					&format!("+ Wrote template: {}", target_path.display()),
				);
			}
		}
	}
}
