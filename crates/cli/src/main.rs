//! `vane` — operator CLI for the `vaned` daemon. Speaks the management
//! protocol over the daemon's Unix socket. Two output modes: a
//! pretty-printer for humans (default) and `--json` for scripts.
//!
//! See `spec/architecture/16-crate-layout.md` § _CLI_.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, Subcommand};
use vane_core::version::{BuildInfo, format_version};
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{
	CompileDryRunArgs, CompileDryRunResult, ConnectionInfo, GetActiveConfigResult,
	ListConnectionsResult, ListenerStatus, NoArgs, PingResult, ReloadResult, ShutdownResult,
	StatsResult, VERB_COMPILE_DRY_RUN, VERB_GET_ACTIVE_CONFIG, VERB_LIST_CONNECTIONS, VERB_PING,
	VERB_RELOAD, VERB_SHUTDOWN, VERB_STATS, VERB_TAIL_FLOW_LOG, VERB_TAIL_LOG,
};

const BUILD_INFO: BuildInfo = BuildInfo {
	version: env!("CARGO_PKG_VERSION"),
	commit: env!("VANE_COMMIT"),
	build_date: env!("VANE_BUILD_DATE"),
	rustc: env!("VANE_RUSTC"),
	cargo: env!("VANE_CARGO"),
	features: &[],
	protocols: &[],
};

const DEFAULT_SOCKET: &str = "/tmp/vaned.sock";

#[derive(Parser, Debug)]
#[command(
	name = "vane",
	about = "vane proxy CLI",
	version = env!("CARGO_PKG_VERSION"),
	disable_version_flag = true,
)]
struct Cli {
	/// Print build banner (version, commit, build date, toolchain) and exit.
	#[arg(short = 'V', long = "version", global = true)]
	version: bool,

	/// Path to the daemon's mgmt Unix socket. Falls back to
	/// `VANE_MGMT_UNIX` then `/tmp/vaned.sock`.
	#[arg(long, global = true)]
	socket: Option<PathBuf>,

	/// Emit machine-readable JSON instead of human pretty output.
	#[arg(long, global = true)]
	json: bool,

	#[command(subcommand)]
	cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
	/// Liveness check. Returns the daemon's build version.
	Ping,
	/// Daemon stats: uptime, graph hash, per-listener bound state and
	/// in-flight connection counts.
	Stats,
	/// Trigger graceful drain + shutdown. The daemon enters its
	/// 30-second soft-drain window; this CLI returns as soon as the
	/// daemon acknowledges the verb.
	Shutdown,
	/// Dump the active `SymbolicFlowGraph` as JSON. Always JSON —
	/// the graph shape is not amenable to a tabular pretty-print.
	GetActiveConfig,
	/// Trigger the reload pipeline (load → compile → link → swap),
	/// equivalent to a file-watcher event.
	Reload,
	/// Run merge → compile → validate against the given config
	/// directory without affecting the active graph. Output is the
	/// resulting `SymbolicFlowGraph` as JSON.
	Compile {
		/// Currently the only supported mode; flag exists for
		/// future-proofing (e.g. `--apply` would go through `reload`).
		#[arg(long = "dry-run")]
		dry_run: bool,
		/// Filesystem path to the candidate config tree.
		config_dir: PathBuf,
	},
	/// Per-listener in-flight connection counts.
	ListConnections,
	/// Subscribe to the daemon's live `FlowLogEvent` broadcast — one
	/// NDJSON frame per emitted event. Stays connected until the
	/// terminal interrupts (Ctrl-C) or the daemon ends the stream.
	TailFlowLog,
	/// Subscribe to the daemon's live `tracing` event stream — one
	/// NDJSON frame per emitted event (RUST_LOG-gated). Same shape as
	/// `tail-flow-log` from the operator's perspective; the underlying
	/// channel is the daemon's tracing-subscriber stack rather than
	/// per-request `FlowLogEvent`s. Stays connected until Ctrl-C.
	TailLog,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
	let cli = Cli::parse();
	if cli.version {
		print!("{}", format_version(&BUILD_INFO));
		return std::process::ExitCode::SUCCESS;
	}
	let Some(cmd) = cli.cmd else {
		eprintln!("vane: no subcommand — try `vane --help`");
		return std::process::ExitCode::FAILURE;
	};
	let socket = cli
		.socket
		.or_else(|| std::env::var("VANE_MGMT_UNIX").ok().map(PathBuf::from))
		.unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET));
	let client = UnixMgmtClient::new(&socket);

	let result = match cmd {
		Cmd::Ping => run_ping(&client, cli.json).await,
		Cmd::Stats => run_stats(&client, cli.json).await,
		Cmd::Shutdown => run_shutdown(&client, cli.json).await,
		Cmd::GetActiveConfig => run_get_active_config(&client).await,
		Cmd::Reload => run_reload(&client, cli.json).await,
		Cmd::Compile { config_dir, .. } => run_compile_dry_run(&client, &config_dir).await,
		Cmd::ListConnections => run_list_connections(&client, cli.json).await,
		Cmd::TailFlowLog => run_tail_flow_log(&client, cli.json).await,
		Cmd::TailLog => run_tail_log(&client, cli.json).await,
	};
	match result {
		Ok(()) => std::process::ExitCode::SUCCESS,
		Err(e) => {
			eprintln!("vane: {e}");
			std::process::ExitCode::FAILURE
		}
	}
}

async fn run_ping(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: PingResult = client.call(VERB_PING, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		println!("pong (vaned {})", r.version);
	}
	Ok(())
}

async fn run_stats(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: StatsResult = client.call(VERB_STATS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		println!("uptime: {}", format_uptime(Duration::from_millis(r.uptime_ms)));
		println!("graph: {}", abbreviate_hash(&r.graph_version_hash));
		println!("listeners:");
		print_listener_rows(&r.listeners);
	}
	Ok(())
}

async fn run_shutdown(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: ShutdownResult = client.call(VERB_SHUTDOWN, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else if r.draining {
		println!("shutdown signal sent — daemon draining");
	} else {
		println!("shutdown verb returned draining=false (unexpected)");
	}
	Ok(())
}

async fn run_get_active_config(client: &UnixMgmtClient) -> anyhow::Result<()> {
	let r: GetActiveConfigResult = client.call(VERB_GET_ACTIVE_CONFIG, &NoArgs {}).await?;
	// Always JSON — the symbolic graph has no sensible tabular form.
	print_json(&r.graph)?;
	Ok(())
}

async fn run_reload(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: ReloadResult = client.call(VERB_RELOAD, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		match r {
			ReloadResult::Swapped { hash } => {
				println!("swapped (hash={})", abbreviate_hash(&hash));
			}
			ReloadResult::Unchanged { hash } => {
				println!("unchanged (hash={})", abbreviate_hash(&hash));
			}
		}
	}
	Ok(())
}

async fn run_compile_dry_run(client: &UnixMgmtClient, config_dir: &Path) -> anyhow::Result<()> {
	let args = CompileDryRunArgs { config_dir: config_dir.to_string_lossy().into_owned() };
	let r: CompileDryRunResult = client.call(VERB_COMPILE_DRY_RUN, &args).await?;
	print_json(&r.graph)?;
	Ok(())
}

async fn run_list_connections(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: ListConnectionsResult = client.call(VERB_LIST_CONNECTIONS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		println!("listeners:");
		print_listener_rows(&r.listeners);
		println!("connections:");
		print_connection_rows(&r.connections);
	}
	Ok(())
}

async fn run_tail_flow_log(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	// Race the streaming call against Ctrl-C. The streaming verb returns
	// `Ok(())` on a clean End frame; Ctrl-C aborts the future, which
	// drops the socket and lets the daemon notice the disconnect.
	let stream_fut = client.call_stream(VERB_TAIL_FLOW_LOG, &NoArgs {}, |frame| {
		if json {
			// One NDJSON line per event — operators pipe to `jq -c .`
			// or similar. Encoding failures fall back to a debug print
			// rather than tearing the stream down.
			match serde_json::to_string(&frame) {
				Ok(s) => println!("{s}"),
				Err(e) => eprintln!("vane: encode error: {e}"),
			}
		} else {
			print_flow_event_pretty(&frame);
		}
	});
	tokio::select! {
		result = stream_fut => Ok(result?),
		_ = tokio::signal::ctrl_c() => {
			// Drop the future so its socket closes; the daemon will see
			// the disconnect and stop pushing events. We exit `Ok` so
			// shells don't show an error on the operator-initiated cancel.
			Ok(())
		}
	}
}

async fn run_tail_log(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	// Race the streaming call against Ctrl-C — same pattern as
	// `tail-flow-log`. Each frame matches the wire shape of
	// `vane_engine::tracing_broadcast::TracingFrame`:
	// `{ t, level, target, message, fields }`.
	let stream_fut = client.call_stream(VERB_TAIL_LOG, &NoArgs {}, |frame| {
		if json {
			match serde_json::to_string(&frame) {
				Ok(s) => println!("{s}"),
				Err(e) => eprintln!("vane: encode error: {e}"),
			}
		} else {
			print_tracing_frame_pretty(&frame);
		}
	});
	tokio::select! {
		result = stream_fut => Ok(result?),
		_ = tokio::signal::ctrl_c() => Ok(()),
	}
}

/// Render one `TracingFrame`-shaped JSON value as a human-readable row.
/// Format: `HH:MM:SS.mmm  LEVEL  target: message {key=value, …}`.
fn print_tracing_frame_pretty(frame: &serde_json::Value) {
	let t_ms = frame.get("t").and_then(serde_json::Value::as_u64).unwrap_or(0);
	let level = frame.get("level").and_then(serde_json::Value::as_str).unwrap_or("?");
	let target = frame.get("target").and_then(serde_json::Value::as_str).unwrap_or("?");
	let message = frame.get("message").and_then(serde_json::Value::as_str).unwrap_or("");
	let fields_render = frame
		.get("fields")
		.and_then(serde_json::Value::as_object)
		.filter(|m| !m.is_empty())
		.map(render_fields)
		.unwrap_or_default();
	let ts = format_unix_ms_clock(t_ms);
	println!("{ts}  {level:<5}  {target}: {message}{fields_render}");
}

/// Render `key=value` pairs joined by spaces, prefixed by a space when
/// non-empty. Strings render verbatim (without surrounding quotes);
/// other types use their JSON form.
fn render_fields(map: &serde_json::Map<String, serde_json::Value>) -> String {
	let mut out = String::with_capacity(64);
	for (k, v) in map {
		out.push(' ');
		out.push_str(k);
		out.push('=');
		match v {
			serde_json::Value::String(s) => out.push_str(s),
			other => out.push_str(&other.to_string()),
		}
	}
	out
}

/// Format a Unix millis timestamp as `HH:MM:SS.mmm` in UTC. Avoids
/// pulling in `chrono` for one format call — `tail-log` doesn't need
/// timezone-aware rendering, just a stable wall-clock anchor.
fn format_unix_ms_clock(ms: u64) -> String {
	let secs = ms / 1_000;
	let millis = ms % 1_000;
	let hour = (secs / 3_600) % 24;
	let minute = (secs / 60) % 60;
	let second = secs % 60;
	format!("{hour:02}:{minute:02}:{second:02}.{millis:03}")
}

/// Render one `FlowLogEvent`-shaped JSON value as a human-readable row.
/// Falls back to JSON for the `data` blob since its shape varies per
/// `kind` (`Trajectory` carries a list of steps, `Error` a serialized
/// error, etc.).
fn print_flow_event_pretty(frame: &serde_json::Value) {
	let kind = frame.get("kind").and_then(serde_json::Value::as_str).unwrap_or("?");
	let conn = frame.get("conn").and_then(serde_json::Value::as_u64).unwrap_or(0);
	let t = frame.get("t").and_then(serde_json::Value::as_u64).unwrap_or(0);
	let seq = frame.get("seq").and_then(serde_json::Value::as_u64).unwrap_or(0);
	let node = frame
		.get("node")
		.and_then(serde_json::Value::as_u64)
		.map(|n| format!(" node={n}"))
		.unwrap_or_default();
	let data =
		frame.get("data").filter(|v| !v.is_null()).map(|v| format!(" data={v}")).unwrap_or_default();
	println!("t={t:>13} conn={conn:016x} seq={seq:>3} kind={kind}{node}{data}");
}

fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
	let s = serde_json::to_string_pretty(value)?;
	println!("{s}");
	Ok(())
}

fn print_listener_rows(rows: &[ListenerStatus]) {
	if rows.is_empty() {
		println!("  (none)");
		return;
	}
	let max_addr_width = rows.iter().map(|r| r.addr.len()).max().unwrap_or(0);
	for row in rows {
		let state = if row.bound { "bound" } else { "down" };
		println!(
			"  {addr:<width$}  {state:<5}  in_flight={count}",
			addr = row.addr,
			width = max_addr_width,
			state = state,
			count = row.in_flight_count,
		);
	}
}

fn print_connection_rows(rows: &[ConnectionInfo]) {
	if rows.is_empty() {
		println!("  (none)");
		return;
	}
	let max_remote = rows.iter().map(|r| r.remote.len()).max().unwrap_or(0);
	let max_listener = rows.iter().map(|r| r.listener_addr.len()).max().unwrap_or(0);
	for row in rows {
		println!(
			"  {conn_id}  {remote:<rw$} → {listener:<lw$}  age={age}",
			conn_id = row.conn_id,
			remote = row.remote,
			rw = max_remote,
			listener = row.listener_addr,
			lw = max_listener,
			age = format_age_ms(row.age_ms),
		);
	}
}

/// Compact age renderer for CLI rows. Falls back to ms / s / m+s
/// depending on magnitude so long-lived connections show "5m 12s"
/// rather than "312123ms".
fn format_age_ms(ms: u64) -> String {
	if ms < 1_000 { format!("{ms}ms") } else { format_uptime(Duration::from_millis(ms)) }
}

/// Render a SHA-256 hash with the leading 12 hex chars + ellipsis. Full
/// 64-char hash is always available via `--json`; pretty mode trades
/// theoretical uniqueness for a tighter line.
fn abbreviate_hash(hex: &str) -> String {
	if hex.len() <= 12 { hex.to_string() } else { format!("{}...", &hex[..12]) }
}

/// Compact uptime renderer. Picks the most significant unit and drops
/// the smaller-unit columns above it. Examples:
///   `Duration::from_secs(45)`        → `"45s"`
///   `Duration::from_secs(180)`       → `"3m 0s"`
///   `Duration::from_secs(3725)`      → `"1h 2m 5s"`
///   `Duration::from_secs(86_400)`    → `"1d 0h 0m 0s"`
fn format_uptime(d: Duration) -> String {
	let total = d.as_secs();
	let days = total / 86_400;
	let hours = (total % 86_400) / 3600;
	let mins = (total % 3600) / 60;
	let secs = total % 60;
	if days > 0 {
		format!("{days}d {hours}h {mins}m {secs}s")
	} else if hours > 0 {
		format!("{hours}h {mins}m {secs}s")
	} else if mins > 0 {
		format!("{mins}m {secs}s")
	} else {
		format!("{secs}s")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn format_uptime_picks_largest_unit() {
		assert_eq!(format_uptime(Duration::from_secs(0)), "0s");
		assert_eq!(format_uptime(Duration::from_secs(45)), "45s");
		assert_eq!(format_uptime(Duration::from_mins(3)), "3m 0s");
		assert_eq!(format_uptime(Duration::from_secs(3725)), "1h 2m 5s");
		assert_eq!(format_uptime(Duration::from_hours(24)), "1d 0h 0m 0s");
		assert_eq!(format_uptime(Duration::from_secs(90_061)), "1d 1h 1m 1s");
	}

	#[test]
	fn format_age_ms_picks_unit_by_magnitude() {
		assert_eq!(format_age_ms(0), "0ms");
		assert_eq!(format_age_ms(345), "345ms");
		assert_eq!(format_age_ms(1_500), "1s");
		assert_eq!(format_age_ms(60_500), "1m 0s");
	}

	#[test]
	fn abbreviate_hash_handles_short_and_long() {
		assert_eq!(abbreviate_hash("abc"), "abc");
		assert_eq!(abbreviate_hash("a".repeat(12).as_str()), "a".repeat(12));
		assert_eq!(abbreviate_hash("abcdef0123456789"), "abcdef012345...");
	}
}
