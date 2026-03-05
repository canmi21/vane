use anyhow::Result;
use bytes::Bytes;
use std::fmt::Write;
use tokio::fs;

pub async fn generate_listing(root_path: &std::path::Path, uri_path: &str) -> Result<Bytes> {
	let mut entries = fs::read_dir(root_path).await?;
	let mut html = String::new();

	write!(
		html,
		"<html><head><title>Index of {uri_path}</title></head>"
	)?;
	write!(html, "<body><h1>Index of {uri_path}</h1><hr><table>")?;
	write!(html, "<tr><th>Name</th><th>Size</th></tr>")?;

	// Add Parent Link
	if uri_path != "/" {
		html.push_str("<tr><td><a href=\"..\">../</a></td><td>-</td></tr>");
	}

	while let Ok(Some(entry)) = entries.next_entry().await {
		let meta = entry.metadata().await?;
		let name = entry.file_name().to_string_lossy().to_string();
		let is_dir = meta.is_dir();

		let display_name = if is_dir {
			format!("{name}/")
		} else {
			name.clone()
		};
		let size_str = if is_dir {
			"-".to_owned()
		} else {
			format!("{}", meta.len())
		};

		write!(
			html,
			"<tr><td><a href=\"{name}\">{display_name}</a></td><td>{size_str}</td></tr>"
		)?;
	}

	html.push_str("</table><hr></body></html>");
	Ok(Bytes::from(html))
}
