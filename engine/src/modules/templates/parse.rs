/* engine/src/modules/templates/parse.rs */

use crate::daemon::config;
use std::collections::HashMap;

/// Renders an HTML template by replacing placeholders.
///
/// Reads a template file from the `templates` directory, finds all placeholders
/// formatted as `%KEY%`, and replaces them with the corresponding values
/// from the `substitutions` map.
pub async fn render_template(
	file_name: &str,
	substitutions: &HashMap<String, String>,
) -> Result<String, String> {
	let mut path = config::get_templates_dir();
	path.push(format!("{}.html", file_name));

	let mut content = tokio::fs::read_to_string(&path)
		.await
		.map_err(|e| format!("Failed to read template file '{}': {}", path.display(), e))?;

	// Iterate through the provided key-value pairs and replace placeholders.
	for (key, value) in substitutions {
		let placeholder = format!("%{}%", key.to_uppercase());
		content = content.replace(&placeholder, value);
	}

	Ok(content)
}
