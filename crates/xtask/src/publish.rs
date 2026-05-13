// Workspace-wide crates.io publish workflow:
//
//   plan  - emit topological order, marking each crate `skip` (already
//           on crates.io at this version) or `publish`.
//   dry   - `cargo publish --dry-run` per crate. Crates whose workspace
//           deps are all already on the registry verify the package
//           build; the rest go pack-only via `--no-verify` since their
//           unpublished siblings aren't resolvable from the registry.
//   run   - real `cargo publish` per crate, polling the sparse index
//           between dependents so each new version is visible before
//           the next dependent's verify-build runs.
//
// `run` runs `just gate` first (skip with `--skip-gate`) and requires
// crates.io auth via either `CARGO_REGISTRY_TOKEN` in the environment
// or a token previously written by `cargo login` into
// `$CARGO_HOME/credentials.toml` (defaults to `~/.cargo/credentials.toml`).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use cargo_metadata::MetadataCommand;
use serde::Serialize;

#[derive(Clone, Copy)]
pub(crate) enum Mode {
	Dry,
	Real,
}

#[derive(Serialize, Clone)]
struct PlanRow {
	action: Action,
	name: String,
	version: String,
	manifest: PathBuf,
	deps: Vec<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Action {
	Skip,
	Publish,
}

pub(crate) fn plan(only: Option<&str>, json: bool) -> Result<()> {
	let plan = compute_plan(only)?;
	if json {
		emit_json(&plan)?;
	} else {
		emit_table(&plan);
	}
	Ok(())
}

pub(crate) fn run(mode: Mode, only: Option<&str>, skip_gate: bool) -> Result<()> {
	if matches!(mode, Mode::Real)
		&& std::env::var_os("CARGO_REGISTRY_TOKEN").is_none()
		&& !has_cargo_login_token()
	{
		bail!(
			"real publish needs crates.io auth: set CARGO_REGISTRY_TOKEN or run `cargo login` \
			 to write the token into $CARGO_HOME/credentials.toml"
		);
	}
	if matches!(mode, Mode::Real) {
		if skip_gate {
			eprintln!("xtask publish: gate skipped via --skip-gate");
		} else {
			eprintln!("xtask publish: running just gate ...");
			let status = Command::new("just").arg("gate").status().context("invoking `just gate`")?;
			if !status.success() {
				bail!("`just gate` failed");
			}
		}
	}

	let plan = compute_plan(only)?;
	dispatch(&plan, mode)
}

fn compute_plan(only: Option<&str>) -> Result<Vec<PlanRow>> {
	let metadata = load_metadata()?;

	// cargo_metadata 0.23 wraps Package.name in a `PackageName` newtype;
	// flatten to `String` at the boundary so the rest of this module
	// keeps working with plain strings.
	let publishable: BTreeMap<String, &cargo_metadata::Package> = metadata
		.packages
		.iter()
		.filter(|p| !matches!(p.publish.as_deref(), Some(&[])))
		.map(|p| (p.name.to_string(), p))
		.collect();

	let deps: BTreeMap<String, Vec<String>> = publishable
		.values()
		.map(|p| {
			let mut seen = HashSet::new();
			let mut out = Vec::new();
			for d in &p.dependencies {
				if matches!(d.kind, cargo_metadata::DependencyKind::Development) {
					continue;
				}
				if !publishable.contains_key(&d.name) {
					continue;
				}
				if seen.insert(d.name.clone()) {
					out.push(d.name.clone());
				}
			}
			(p.name.to_string(), out)
		})
		.collect();

	let order = topo_sort(&deps).context("dependency cycle in workspace")?;

	let candidates: Vec<&String> =
		order.iter().filter(|n| only.is_none_or(|f| f == n.as_str())).collect();
	let total = candidates.len();
	eprintln!(
		"xtask publish: querying sparse index for {total} workspace crate{}...",
		if total == 1 { "" } else { "s" },
	);

	// Sparse-index queries are the dominant wall time of `xtask publish
	// plan`. Each lookup is a small TLS GET that spends almost all of
	// its time blocked on network round-trips, so fanning them out
	// across scoped threads cuts the precondition step from
	// O(N × latency) to ~O(latency). The sparse index is a high-
	// throughput CloudFlare-backed CDN; ~30 concurrent GETs is well
	// inside its budget and matches what `cargo` itself does on a
	// `cargo update`.
	//
	// Progress lines remain serialised through `stderr_lock` so they
	// don't tear, but appear in completion order (each line names
	// the crate so the order is unambiguous). The completion counter
	// is monotonic; the per-row results land back in the original
	// topo order via the indexed `results` slot.
	let results: Vec<Mutex<Option<Result<Action>>>> = (0..total).map(|_| Mutex::new(None)).collect();
	let counter = AtomicUsize::new(0);
	let stderr_lock = Mutex::new(());
	thread::scope(|s| {
		for (i, name) in candidates.iter().enumerate() {
			let pkg = publishable[name.as_str()];
			let version = pkg.version.to_string();
			let name = (*name).clone();
			let results = &results;
			let counter = &counter;
			let stderr_lock = &stderr_lock;
			s.spawn(move || {
				let outcome = version_published(&name, &version)
					.map(|seen| if seen { Action::Skip } else { Action::Publish });
				let done = counter.fetch_add(1, Ordering::SeqCst) + 1;
				let _stderr = stderr_lock.lock();
				match &outcome {
					Ok(Action::Skip) => {
						eprintln!("  [check] [{done}/{total}] {name} {version} → on-registry");
					}
					Ok(Action::Publish) => {
						eprintln!("  [check] [{done}/{total}] {name} {version} → needs-publish");
					}
					Err(e) => {
						eprintln!("  [check] [{done}/{total}] {name} {version} → error: {e:#}");
					}
				}
				*results[i].lock().expect("results slot poisoned") = Some(outcome);
			});
		}
	});

	let mut plan = Vec::new();
	for (i, name) in candidates.iter().enumerate() {
		let pkg = publishable[name.as_str()];
		let action = results[i]
			.lock()
			.expect("results slot poisoned")
			.take()
			.expect("scoped thread always sets its slot before scope exits")?;
		plan.push(PlanRow {
			action,
			name: (*name).clone(),
			version: pkg.version.to_string(),
			manifest: pkg.manifest_path.clone().into_std_path_buf(),
			deps: deps[name.as_str()].clone(),
		});
	}

	if let Some(filter) = only
		&& plan.is_empty()
	{
		bail!("--only={filter} matched no publishable crate");
	}
	Ok(plan)
}

// Run `cargo metadata` ourselves and feed the JSON to cargo_metadata's
// parser. Equivalent to `MetadataCommand::new().no_deps().exec()` but
// gives us full control over the spawned process.
fn load_metadata() -> Result<cargo_metadata::Metadata> {
	let output = Command::new("cargo")
		.args(["metadata", "--format-version", "1", "--no-deps"])
		.output()
		.context("invoking `cargo metadata`")?;
	if !output.status.success() {
		bail!("`cargo metadata` exited non-zero");
	}
	let stdout = std::str::from_utf8(&output.stdout).context("non-utf8 cargo metadata output")?;
	MetadataCommand::parse(stdout).context("parsing cargo metadata output")
}

fn topo_sort(deps: &BTreeMap<String, Vec<String>>) -> Result<Vec<String>> {
	let mut remaining: BTreeMap<String, BTreeSet<String>> =
		deps.iter().map(|(n, ds)| (n.clone(), ds.iter().cloned().collect())).collect();
	let mut order = Vec::new();
	while !remaining.is_empty() {
		let ready: Vec<String> =
			remaining.iter().filter(|(_, ds)| ds.is_empty()).map(|(n, _)| n.clone()).collect();
		if ready.is_empty() {
			let stuck: Vec<&str> = remaining.keys().map(String::as_str).collect();
			bail!("dependency cycle among: {}", stuck.join(", "));
		}
		for n in &ready {
			remaining.remove(n);
		}
		for ds in remaining.values_mut() {
			for n in &ready {
				ds.remove(n);
			}
		}
		order.extend(ready);
	}
	Ok(order)
}

fn emit_table(plan: &[PlanRow]) {
	println!();
	println!("  PLAN");
	println!("  {:<9} {:<30} VERSION", "ACTION", "CRATE");
	println!("  {}", "-".repeat(52));
	for row in plan {
		let tag = match row.action {
			Action::Skip => "[skip]",
			Action::Publish => "[publish]",
		};
		println!("  {tag:<9} {:<30} {}", row.name, row.version);
	}
	println!();
}

fn emit_json(plan: &[PlanRow]) -> Result<()> {
	for row in plan {
		println!("{}", serde_json::to_string(row)?);
	}
	Ok(())
}

fn dispatch(plan: &[PlanRow], mode: Mode) -> Result<()> {
	let mut available: HashSet<String> =
		plan.iter().filter(|r| r.action == Action::Skip).map(|r| r.name.clone()).collect();

	for row in plan {
		if row.action == Action::Skip {
			println!("  [skip]    {} {}", row.name, row.version);
			continue;
		}

		let all_avail = row.deps.iter().all(|d| available.contains(d));
		match mode {
			Mode::Dry => {
				let manifest = row.manifest.to_string_lossy();
				let mut args: Vec<&str> = vec!["publish", "--dry-run", "--allow-dirty"];
				if all_avail {
					println!("  [dry]     {} {} (verify)", row.name, row.version);
				} else {
					args.push("--no-verify");
					println!("  [dry]     {} {} (no-verify, unpublished sibling)", row.name, row.version);
				}
				args.push("--manifest-path");
				args.push(&manifest);
				let status = Command::new("cargo").args(&args).status()?;
				if !status.success() {
					bail!("dry-run failed for {}", row.name);
				}
			}
			Mode::Real => {
				if !all_avail {
					bail!("{} has unpublished workspace deps in this run; topo sort bug", row.name);
				}
				println!("  [publish] {} {}", row.name, row.version);
				let status = Command::new("cargo")
					.args(["publish", "--manifest-path", &row.manifest.to_string_lossy()])
					.status()?;
				if !status.success() {
					bail!("publish failed for {}", row.name);
				}
				wait_for_index(&row.name, &row.version)?;
				available.insert(row.name.clone());
			}
		}
	}

	let label = match mode {
		Mode::Dry => "dry",
		Mode::Real => "real",
	};
	println!("\nxtask publish: {label}-run complete");
	Ok(())
}

// Capped exponential backoff: 2s start, 10s max, 60s deadline.
// `cargo publish` already waits for upload by default since 1.66;
// this is the explicit gate before the next dependent's verify-build.
fn wait_for_index(name: &str, version: &str) -> Result<()> {
	let mut interval = Duration::from_secs(2);
	let mut elapsed = Duration::ZERO;
	let deadline = Duration::from_mins(1);
	eprintln!("  [wait]    {name} {version} (sparse index propagation)");
	while elapsed < deadline {
		if version_published(name, version)? {
			eprintln!("  [seen]    {name} {version} ({}s)", elapsed.as_secs());
			return Ok(());
		}
		thread::sleep(interval);
		elapsed += interval;
		eprintln!(
			"  [wait]    {name} {version} ({}s elapsed, retry in {}s)",
			elapsed.as_secs(),
			interval.as_secs()
		);
		let bump = (interval / 2).max(Duration::from_secs(1));
		interval = (interval + bump).min(Duration::from_secs(10));
	}
	bail!("timeout waiting for {name}@{version} on sparse index")
}

fn version_published(name: &str, version: &str) -> Result<bool> {
	let url = format!("https://index.crates.io/{}", index_path(name));
	// ureq 3 moved per-request timeout from `RequestBuilder::timeout`
	// to a `RequestBuilder::config()` builder chain; the 404 variant
	// also flattened from `Error::Status(code, response)` to
	// `Error::StatusCode(code)` (responses for non-success codes are
	// no longer carried on the error path by default).
	let mut response =
		match ureq::get(&url).config().timeout_global(Some(Duration::from_secs(15))).build().call() {
			Ok(r) => r,
			Err(ureq::Error::StatusCode(404)) => return Ok(false),
			Err(e) => {
				return Err(anyhow::Error::new(e).context(format!("sparse index lookup for {name}")));
			}
		};
	let body = response.body_mut().read_to_string().context("reading sparse index body")?;
	for line in body.lines() {
		let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
			continue;
		};
		if value.get("vers").and_then(|v| v.as_str()) == Some(version) {
			return Ok(true);
		}
	}
	Ok(false)
}

fn index_path(name: &str) -> String {
	match name.len() {
		0 => unreachable!("crate names cannot be empty"),
		1 => format!("1/{name}"),
		2 => format!("2/{name}"),
		3 => format!("3/{}/{name}", &name[..1]),
		_ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
	}
}

/// Locate `credentials.toml` honouring `$CARGO_HOME` (Cargo's documented
/// override) and falling back to `$HOME/.cargo/credentials.toml`.
fn cargo_credentials_path() -> Option<PathBuf> {
	if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
		return Some(PathBuf::from(cargo_home).join("credentials.toml"));
	}
	let home = std::env::var_os("HOME")?;
	Some(PathBuf::from(home).join(".cargo").join("credentials.toml"))
}

/// Whether `cargo login` has written a usable crates.io token into
/// `credentials.toml`. Cargo recognises two layouts and either
/// satisfies the publish path:
///
/// * `[registry]` with a `token = "..."` field (older `cargo login` form).
/// * `[registries.crates-io]` with a `token = "..."` field (newer form
///   produced by `cargo login --registry crates-io` or recent cargo
///   versions).
///
/// Empty / quoted-empty token strings count as missing so an operator
/// who blanked the file doesn't silently bypass the precondition.
fn has_cargo_login_token() -> bool {
	let Some(path) = cargo_credentials_path() else { return false };
	let Ok(text) = std::fs::read_to_string(&path) else { return false };
	let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return false };
	let nonempty_token = |item: Option<&toml_edit::Item>| -> bool {
		item.and_then(toml_edit::Item::as_str).is_some_and(|s| !s.is_empty())
	};
	if let Some(reg) = doc.get("registry").and_then(toml_edit::Item::as_table)
		&& nonempty_token(reg.get("token"))
	{
		return true;
	}
	if let Some(crates_io) = doc
		.get("registries")
		.and_then(toml_edit::Item::as_table)
		.and_then(|t| t.get("crates-io"))
		.and_then(toml_edit::Item::as_table)
		&& nonempty_token(crates_io.get("token"))
	{
		return true;
	}
	false
}
