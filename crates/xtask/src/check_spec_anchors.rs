// Verify spec-anchor references in workspace source resolve to real
// headings in their target spec file. Anchor syntax is documented in
// spec/conventions.md; this checker enforces it.
//
// The matcher is per-comment-block and position-aware:
//
// - Contiguous line-comment lines are folded into one logical block
//   (prefix stripped, joined with spaces) so anchor tokens that wrap
//   across two lines are seen as one logical token.
// - Each section anchor is paired with the closest preceding spec
//   path mention in the same block; if none, the most recent mention
//   from the previous 30 source lines carries forward.
// - Slash continuations after a primary anchor are treated as sibling
//   sections under the same spec file as the primary.
// - Two same-section primary anchors with only whitespace between
//   them are reported as a duplicate regression. Same-section
//   references that are separated by prose (or by a different anchor)
//   are not flagged — they're assumed intentional cross-references.
// - Heading match is exact — no substring fallback, no near-match.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fancy_regex::Regex;

use crate::workspace;

pub(crate) fn run() -> Result<()> {
	let root = workspace::root()?;

	let headings = build_heading_index(&root.join("spec"), &root)?;
	let sources = collect_rust_sources(&root.join("crates"), &root)?;

	let heading_re = Regex::new(r"^\s*//[!/]?[ \t]?(.*?)\s*$").unwrap();
	let token_re = Regex::new(concat!(
		r"(spec/[A-Za-z0-9_/-]+\.md)",
		r"|",
		r"§\s+_([^_\n][^_\n]*?)_(?=[\s.,;:)\]/`*]|\z)",
		r"|",
		r"/\s+_([^_\n][^_\n]*?)_(?=[\s.,;:)\]/`*]|\z)",
	))
	.unwrap();

	let mut total: usize = 0;
	let mut broken: BTreeMap<BrokenKey, Vec<String>> = BTreeMap::new();

	for src in &sources {
		let abs = root.join(src);
		let content = fs::read_to_string(&abs).with_context(|| format!("reading {}", abs.display()))?;
		scan_file(src, &content, &heading_re, &token_re, &headings, &mut total, &mut broken)?;
	}

	let broken_count: usize = broken.values().map(Vec::len).sum();
	println!("Total scanned: {total}");
	println!("Broken: {broken_count}");

	if broken_count == 0 {
		return Ok(());
	}

	println!();
	let mut entries: Vec<_> = broken.iter().collect();
	entries.sort_by(|(ka, va), (kb, vb)| vb.len().cmp(&va.len()).then_with(|| ka.cmp(kb)));
	for (key, sites) in entries {
		let plural = if sites.len() == 1 { "" } else { "s" };
		println!(
			"  [{}] {} § _{}_  ({} site{plural})",
			key.kind.label(),
			key.spec_file,
			key.section,
			sites.len(),
		);
		for site in sites {
			println!("    {site}");
		}
	}
	bail!("{broken_count} broken spec anchor reference(s)");
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct BrokenKey {
	kind: BrokenKind,
	spec_file: String,
	section: String,
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum BrokenKind {
	Duplicate,
	NoFile,
	MissingFile,
	MissingHeading,
}

impl BrokenKind {
	fn label(&self) -> &'static str {
		match self {
			Self::Duplicate => "duplicate",
			Self::NoFile => "no spec file in scope",
			Self::MissingFile => "spec file not found",
			Self::MissingHeading => "heading not in spec file",
		}
	}
}

fn build_heading_index(spec_dir: &Path, root: &Path) -> Result<HashMap<String, HashSet<String>>> {
	let head_re = Regex::new(r"^#{1,6}\s+(.+?)\s*$").unwrap();
	let mut headings: HashMap<String, HashSet<String>> = HashMap::new();
	for path in walk_files(spec_dir, "md")? {
		let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
		let content =
			fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
		let entry = headings.entry(rel).or_default();
		for line in content.lines() {
			if let Ok(Some(c)) = head_re.captures(line)
				&& let Some(h) = c.get(1)
			{
				entry.insert(h.as_str().to_string());
			}
		}
	}
	Ok(headings)
}

fn collect_rust_sources(crates_dir: &Path, root: &Path) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	for path in walk_files(crates_dir, "rs")? {
		if path.components().any(|c| c.as_os_str() == "target") {
			continue;
		}
		let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
		out.push(rel);
	}
	out.sort();
	Ok(out)
}

fn walk_files(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	walk_into(dir, ext, &mut out)?;
	Ok(out)
}

fn walk_into(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
	if !dir.is_dir() {
		return Ok(());
	}
	for entry in fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
		let entry = entry?;
		let path = entry.path();
		if path.is_dir() {
			walk_into(&path, ext, out)?;
		} else if path.extension().and_then(|s| s.to_str()) == Some(ext) {
			out.push(path);
		}
	}
	Ok(())
}

struct CommentBlock {
	text: String,
	// One offset per byte of `text` (plus one trailing slot for the
	// joiner), each entry the 0-indexed source line that byte came
	// from. `text.len() <= offset_to_line.len()`.
	offset_to_line: Vec<usize>,
}

fn group_comment_blocks(content: &str, comment_re: &Regex) -> Vec<CommentBlock> {
	let mut blocks: Vec<CommentBlock> = Vec::new();
	let mut text = String::new();
	let mut offsets: Vec<usize> = Vec::new();
	let mut started = false;

	for (i, line) in content.lines().enumerate() {
		let captured = comment_re
			.captures(line)
			.ok()
			.flatten()
			.and_then(|c| c.get(1))
			.map(|m| m.as_str().to_string());
		if let Some(body) = captured {
			if started {
				text.push(' ');
			}
			text.push_str(&body);
			for _ in 0..body.len() {
				offsets.push(i);
			}
			offsets.push(i);
			started = true;
		} else if started {
			blocks.push(CommentBlock {
				text: std::mem::take(&mut text),
				offset_to_line: std::mem::take(&mut offsets),
			});
			started = false;
		}
	}
	if started {
		blocks.push(CommentBlock { text, offset_to_line: offsets });
	}
	blocks
}

fn scan_file(
	src: &Path,
	content: &str,
	comment_re: &Regex,
	token_re: &Regex,
	headings: &HashMap<String, HashSet<String>>,
	total: &mut usize,
	broken: &mut BTreeMap<BrokenKey, Vec<String>>,
) -> Result<()> {
	let blocks = group_comment_blocks(content, comment_re);
	// Carry the most recent spec path mention forward across blocks
	// when the gap is at most 30 source lines; `None` means "no
	// reference to carry yet."
	let mut carry: Option<(String, usize)> = None;
	let src_str = src.to_string_lossy();

	for block in &blocks {
		let first_line = block.offset_to_line.first().copied().unwrap_or(0);
		let mut current = match &carry {
			Some((path, line)) if first_line.saturating_sub(*line) <= 30 => Some(path.clone()),
			_ => None,
		};
		// Tracks the most recent primary anchor: section name plus the
		// byte offset right after its closing `_`. Used to flag adjacent
		// duplicates only when nothing but whitespace sits between the
		// two matches.
		let mut last_primary: Option<(String, usize)> = None;

		for cap in token_re.captures_iter(&block.text) {
			let cap = cap.context("regex capture iteration failed")?;
			let whole = cap.get(0).unwrap();
			let pos = whole.start();
			let line_num = block.offset_to_line.get(pos).copied().unwrap_or(first_line) + 1;

			if let Some(spec) = cap.get(1) {
				let path = spec.as_str().to_string();
				current = Some(path.clone());
				let at_line = block.offset_to_line.get(pos).copied().unwrap_or(first_line);
				carry = Some((path, at_line));
				last_primary = None;
				continue;
			}

			let primary = cap.get(2);
			let cont = cap.get(3);
			let Some(matched) = primary.or(cont) else {
				continue;
			};

			let mut sec = matched.as_str().replace(['\n', '\t'], " ");
			while sec.contains("  ") {
				sec = sec.replace("  ", " ");
			}
			sec = sec.trim_end_matches('.').to_string();

			*total += 1;

			if primary.is_some() {
				if let Some((prev_sec, prev_end)) = &last_primary
					&& prev_sec == &sec
					&& block.text[*prev_end..pos].chars().all(char::is_whitespace)
				{
					broken
						.entry(BrokenKey {
							kind: BrokenKind::Duplicate,
							spec_file: current.clone().unwrap_or_else(|| "<no-file>".into()),
							section: sec.clone(),
						})
						.or_default()
						.push(format!("{src_str}:{line_num}"));
				}
				last_primary = Some((sec.clone(), whole.end()));
			}

			let Some(spec_path) = current.as_ref() else {
				broken
					.entry(BrokenKey {
						kind: BrokenKind::NoFile,
						spec_file: "<no-file>".into(),
						section: sec.clone(),
					})
					.or_default()
					.push(format!("{src_str}:{line_num}"));
				continue;
			};

			let Some(spec_headings) = headings.get(spec_path) else {
				broken
					.entry(BrokenKey {
						kind: BrokenKind::MissingFile,
						spec_file: spec_path.clone(),
						section: sec.clone(),
					})
					.or_default()
					.push(format!("{src_str}:{line_num}"));
				continue;
			};

			if !spec_headings.contains(&sec) {
				broken
					.entry(BrokenKey {
						kind: BrokenKind::MissingHeading,
						spec_file: spec_path.clone(),
						section: sec.clone(),
					})
					.or_default()
					.push(format!("{src_str}:{line_num}"));
			}
		}
	}
	Ok(())
}
