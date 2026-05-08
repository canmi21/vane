//! `vane` — operator CLI for the `vaned` daemon. Speaks the management
//! protocol over the daemon's Unix socket. Two output modes: a
//! pretty-printer for humans (default) and `--json` for scripts.
//!
//! See [`spec/crates/cli.md` § _Subcommand layout_](../../../spec/crates/cli.md#subcommand-layout).

#[cfg(feature = "tui")]
mod tui;

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Parser, Subcommand};
use owo_colors::{OwoColorize, Stream, Style};
use vane_core::version::{BuildInfo, print_banner};
use vane_mgmt::UnixMgmtClient;
use vane_mgmt::verb::{
	CgiPoolEntry, CompileDryRunArgs, CompileDryRunResult, ConnectionInfo, ForceRenewArgs,
	ForceRenewResult, GetCertsResult, GetConfigResult, GetConnectionsResult, GetMetricsArgs,
	GetMetricsResult, GetPoolsResult, GetUpstreamsResult, ListenerStatus, NoArgs, PingResult,
	PoolDrainArgs, PoolDrainResult, QuicUpstreamEntry, ReloadResult, ShutdownResult, StatsResult,
	TcpUpstreamEntry, VERB_COMPILE_DRY_RUN, VERB_FORCE_RENEW, VERB_GET_CERTS, VERB_GET_CONFIG,
	VERB_GET_CONNECTIONS, VERB_GET_METRICS, VERB_GET_POOLS, VERB_GET_UPSTREAMS, VERB_PING,
	VERB_POOL_DRAIN, VERB_RELOAD, VERB_SHUTDOWN, VERB_STATS, VERB_TAIL_FLOW, VERB_TAIL_LOG,
	WasmPoolEntry,
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

/// Help-text palette for clap. clap 4's built-in styling is bold +
/// underline only; this opts into the `cargo`/`ripgrep`-flavoured
/// modern look (yellow section headers, cyan literals, green
/// placeholders) while still routing through anstyle so non-TTY
/// stdouts get plain text.
const HELP_STYLES: Styles = Styles::styled()
	.header(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
	.usage(AnsiColor::Yellow.on_default().effects(Effects::BOLD))
	.literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
	.placeholder(AnsiColor::Green.on_default());

/// Help template, identical to clap's stock body. The leading blank
/// line comes from `before_help = ""` (an empty `{before-help}`
/// renders as a bare `\n` thanks to clap's auto-spacing); the
/// trailing blank line is added by the manual help printer in
/// `main()` because clap trims whitespace at both ends of
/// `{after-help}`, including unicode space.
const HELP_TEMPLATE: &str =
	"{before-help}{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}{after-help}";

#[derive(Parser, Debug)]
#[command(
	name = "vane",
	about = "Vane — A compact programmable proxy engine",
	version = env!("CARGO_PKG_VERSION"),
	disable_version_flag = true,
	styles = HELP_STYLES,
	help_template = HELP_TEMPLATE,
	before_help = "",
)]
struct Cli {
	/// Print the build banner and exit.
	#[arg(short = 'v', long = "version", global = true)]
	version: bool,

	/// Mgmt Unix socket path (env `VANE_MGMT_UNIX`, default `/tmp/vaned.sock`).
	#[arg(long, global = true)]
	socket: Option<PathBuf>,

	/// Emit JSON instead of human-readable output.
	#[arg(long, global = true)]
	json: bool,

	#[command(subcommand)]
	cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
	/// Liveness check.
	Ping,
	/// Uptime, graph hash, listener summary.
	Stats,
	/// Graceful drain + shutdown.
	Shutdown,
	/// Reload config (compile + swap).
	Reload,
	/// Dry-run compile a config directory; emit the symbolic graph as JSON.
	Compile {
		/// Reserved; today the verb only runs in dry-run mode.
		#[arg(long = "dry-run")]
		dry_run: bool,
		/// Path to the candidate config tree.
		config_dir: PathBuf,
	},
	/// Snapshot a read-only view of the daemon.
	Get {
		#[command(subcommand)]
		what: GetCmd,
	},
	/// Subscribe to a streaming endpoint.
	Tail {
		#[command(subcommand)]
		what: TailCmd,
	},
	/// ACME-managed certificate operations.
	Cert {
		#[command(subcommand)]
		what: CertCmd,
	},
	/// Upstream connection pool operations.
	Pool {
		#[command(subcommand)]
		what: PoolCmd,
	},
	/// Launch the interactive TUI (default action when `vane` is
	/// invoked with no subcommand).
	#[cfg(feature = "tui")]
	Tui,
}

#[derive(Subcommand, Debug)]
enum GetCmd {
	/// Active symbolic flow graph as JSON.
	Config,
	/// In-flight connections snapshot.
	Connections,
	/// Counters and gauges (Prometheus text by default; --json for parsed).
	Metrics,
	/// WASM and CGI pool occupancy.
	Pools,
	/// Cached TCP / TLS / QUIC upstream entries.
	Upstreams,
	/// Tracked managed and static certificates.
	Certs,
}

#[derive(Subcommand, Debug)]
enum TailCmd {
	/// Stream flow-log events.
	Flow,
	/// Stream tracing log frames.
	Log,
}

#[derive(Subcommand, Debug)]
enum CertCmd {
	/// Renew one managed cert now, bypassing the periodic timer.
	Renew {
		/// SNI of the managed cert.
		sni: String,
	},
}

#[derive(Subcommand, Debug)]
enum PoolCmd {
	/// Drop one upstream cache entry by fingerprint id.
	Drain {
		/// 16-char hex id from `vane get upstreams`.
		fingerprint_id: String,
	},
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
	// Pre-clap intercept for the bare `vane --help` / `vane -h` form.
	// clap auto-handles `--help` and exits before main can append
	// anything; routing through `print_help` ourselves lets us add a
	// trailing blank line so the next shell prompt has breathing room.
	// Subcommand help (`vane get --help`, etc.) still goes through
	// clap's default flow — the asymmetry is acceptable and keeps the
	// override logic to the top-level surface.
	let raw: Vec<String> = std::env::args().collect();
	if raw.len() == 2 && (raw[1] == "--help" || raw[1] == "-h") {
		let mut cmd = <Cli as clap::CommandFactory>::command();
		let _ = cmd.print_help();
		println!();
		return std::process::ExitCode::SUCCESS;
	}

	let cli = Cli::parse();
	if cli.version {
		print_banner(&BUILD_INFO);
		return std::process::ExitCode::SUCCESS;
	}
	// Bare `vane` (no subcommand) opens the TUI when the feature is
	// compiled in. With `--no-default-features` the TUI is gone, so
	// we fall back to the original "no subcommand — try --help" hint
	// rather than launching a missing binary path.
	#[cfg(feature = "tui")]
	let cmd = cli.cmd.unwrap_or(Cmd::Tui);
	#[cfg(not(feature = "tui"))]
	let Some(cmd) = cli.cmd else {
		eprintln!(
			"{} no subcommand — try `vane --help`",
			"vane:".if_supports_color(Stream::Stderr, |t| t.style(Style::new().red().bold())),
		);
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
		Cmd::Reload => run_reload(&client, cli.json).await,
		Cmd::Compile { config_dir, .. } => run_compile_dry_run(&client, &config_dir).await,
		Cmd::Get { what: GetCmd::Config } => run_get_config(&client).await,
		Cmd::Get { what: GetCmd::Connections } => run_get_connections(&client, cli.json).await,
		Cmd::Get { what: GetCmd::Metrics } => run_get_metrics(&client, cli.json).await,
		Cmd::Get { what: GetCmd::Pools } => run_get_pools(&client, cli.json).await,
		Cmd::Get { what: GetCmd::Upstreams } => run_get_upstreams(&client, cli.json).await,
		Cmd::Get { what: GetCmd::Certs } => run_get_certs(&client, cli.json).await,
		Cmd::Tail { what: TailCmd::Flow } => run_tail_flow(&client, cli.json).await,
		Cmd::Tail { what: TailCmd::Log } => run_tail_log(&client, cli.json).await,
		Cmd::Cert { what: CertCmd::Renew { sni } } => run_cert_renew(&client, &sni, cli.json).await,
		Cmd::Pool { what: PoolCmd::Drain { fingerprint_id } } => {
			run_pool_drain(&client, &fingerprint_id, cli.json).await
		}
		#[cfg(feature = "tui")]
		Cmd::Tui => tui::run(&BUILD_INFO),
	};
	match result {
		Ok(()) => std::process::ExitCode::SUCCESS,
		Err(e) => {
			eprintln!(
				"{} {e}",
				"vane:".if_supports_color(Stream::Stderr, |t| t.style(Style::new().red().bold()))
			);
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
		print_section("listeners:");
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

async fn run_get_config(client: &UnixMgmtClient) -> anyhow::Result<()> {
	let r: GetConfigResult = client.call(VERB_GET_CONFIG, &NoArgs {}).await?;
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

async fn run_cert_renew(client: &UnixMgmtClient, sni: &str, json: bool) -> anyhow::Result<()> {
	let args = ForceRenewArgs { sni: sni.to_owned() };
	let r: ForceRenewResult = client.call(VERB_FORCE_RENEW, &args).await?;
	if json {
		print_json(&r)?;
	} else if r.queued {
		println!("queued: status={} (sni={sni})", r.current_status);
	} else {
		println!("not queued: sni={sni:?} not declared managed (status={})", r.current_status);
	}
	Ok(())
}

async fn run_get_certs(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: GetCertsResult = client.call(VERB_GET_CERTS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		// One row per cert. The header is bold-styled via print_section
		// so it visually anchors above the data rows; columns themselves
		// are fixed-width on the unstyled string to survive piping.
		let header =
			format!("{:<32} {:<8} {:<10} {:<24} LAST_ERROR", "SNI", "SOURCE", "STATUS", "NOT_AFTER");
		print_section(&header);
		if r.certs.is_empty() {
			print_none_row();
		}
		for entry in &r.certs {
			let na = entry.not_after.as_deref().unwrap_or("-");
			let err = entry.last_error.as_deref().unwrap_or("");
			let status = if entry.status.is_empty() { "-" } else { entry.status.as_str() };
			println!("{:<32} {:<8} {:<10} {:<24} {}", entry.sni, entry.source, status, na, err);
		}
	}
	Ok(())
}

async fn run_get_connections(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: GetConnectionsResult = client.call(VERB_GET_CONNECTIONS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		print_section("listeners:");
		print_listener_rows(&r.listeners);
		print_section("connections:");
		print_connection_rows(&r.connections);
	}
	Ok(())
}

async fn run_get_metrics(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let format = if json { "json" } else { "prometheus" };
	let args = GetMetricsArgs { format: Some(format.to_string()) };
	let r: GetMetricsResult = client.call(VERB_GET_METRICS, &args).await?;
	match r {
		GetMetricsResult::Prometheus { body } => print!("{body}"),
		GetMetricsResult::Json { metrics } => print_json(&metrics)?,
	}
	Ok(())
}

async fn run_get_pools(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: GetPoolsResult = client.call(VERB_GET_POOLS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		print_section("wasm:");
		print_wasm_pool_rows(&r.wasm);
		print_section("cgi:");
		print_cgi_pool_row(r.cgi.as_ref());
	}
	Ok(())
}

async fn run_get_upstreams(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	let r: GetUpstreamsResult = client.call(VERB_GET_UPSTREAMS, &NoArgs {}).await?;
	if json {
		print_json(&r)?;
	} else {
		print_section("tcp:");
		print_tcp_upstream_rows(&r.tcp);
		print_section("quic:");
		print_quic_upstream_rows(&r.quic);
	}
	Ok(())
}

async fn run_pool_drain(
	client: &UnixMgmtClient,
	fingerprint_id: &str,
	json: bool,
) -> anyhow::Result<()> {
	let args = PoolDrainArgs { fingerprint_id: fingerprint_id.to_owned() };
	let r: PoolDrainResult = client.call(VERB_POOL_DRAIN, &args).await?;
	if json {
		print_json(&r)?;
	} else {
		println!("drained: tcp={} quic={}", r.tcp_drained, r.quic_drained);
	}
	Ok(())
}

async fn run_tail_flow(client: &UnixMgmtClient, json: bool) -> anyhow::Result<()> {
	// Race the streaming call against Ctrl-C. The streaming verb returns
	// `Ok(())` on a clean End frame; Ctrl-C aborts the future, which
	// drops the socket and lets the daemon notice the disconnect.
	let stream_fut = client.call_stream(VERB_TAIL_FLOW, &NoArgs {}, |frame| {
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
	// `tail flow`. Each frame matches the wire shape of
	// `tracing_broadcast::TracingFrame`:
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
/// pulling in `chrono` for one format call — `tail log` doesn't need
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

/// Print one section header (`listeners:`, `wasm:`, etc.) — bold on a
/// TTY, plain on a pipe. Centralised so every section uses the same
/// styling and so the color crate import stays in one place.
fn print_section(label: &str) {
	println!("{}", label.if_supports_color(Stream::Stdout, |t| t.bold()));
}

/// Print the "(none)" placeholder rows reach when their data set is
/// empty. Dim on TTY so it visually sinks below real rows.
fn print_none_row() {
	println!("  {}", "(none)".if_supports_color(Stream::Stdout, |t| t.dimmed()));
}

fn print_listener_rows(rows: &[ListenerStatus]) {
	if rows.is_empty() {
		print_none_row();
		return;
	}
	let max_addr_width = rows.iter().map(|r| r.addr.len()).max().unwrap_or(0);
	for row in rows {
		let state_raw = if row.bound { "bound" } else { "down" };
		// Pad to a fixed 5-char column on the *uncolored* string so that
		// columns line up regardless of whether the ANSI escapes get
		// emitted (TTY) or stripped (pipe).
		let padded = format!("{state_raw:<5}");
		let state = if row.bound {
			format!("{}", padded.if_supports_color(Stream::Stdout, |t| t.green()))
		} else {
			format!("{}", padded.if_supports_color(Stream::Stdout, |t| t.red()))
		};
		println!(
			"  {addr:<width$}  {state}  in_flight={count}",
			addr = row.addr,
			width = max_addr_width,
			count = row.in_flight_count,
		);
	}
}

fn print_wasm_pool_rows(rows: &[WasmPoolEntry]) {
	if rows.is_empty() {
		print_none_row();
		return;
	}
	let max_key = rows.iter().map(|r| r.key.len()).max().unwrap_or(0);
	let max_export = rows.iter().map(|r| r.export.len()).max().unwrap_or(0);
	for row in rows {
		println!(
			"  {kind:<10}  {key:<kw$}  {export:<ew$}  cap={cap} in_use={in_use} avail={avail} alloc={alloc} fail={fail}",
			kind = row.kind,
			key = row.key,
			kw = max_key,
			export = row.export,
			ew = max_export,
			cap = row.capacity,
			in_use = row.in_use,
			avail = row.available,
			alloc = row.total_allocations,
			fail = row.failures,
		);
	}
}

fn print_cgi_pool_row(row: Option<&CgiPoolEntry>) {
	match row {
		None => println!("  (cgi disabled or no requests yet)"),
		Some(r) => println!(
			"  cap={cap} in_use={in_use} avail={avail} alloc={alloc} fail={fail}",
			cap = r.cap,
			in_use = r.in_use,
			avail = r.available,
			alloc = r.total_allocations,
			fail = r.failures,
		),
	}
}

fn print_tcp_upstream_rows(rows: &[TcpUpstreamEntry]) {
	if rows.is_empty() {
		print_none_row();
		return;
	}
	for row in rows {
		println!(
			"  {fp}  {scheme}/{version}  alpn=[{alpn}] dns={dns} root={root} verify={verify}",
			fp = row.fingerprint_id,
			scheme = row.scheme,
			version = row.version,
			alpn = row.alpn.join(","),
			dns = row.dns,
			root = row.root_ca,
			verify = row.verify_mode,
		);
	}
}

fn print_quic_upstream_rows(rows: &[QuicUpstreamEntry]) {
	if rows.is_empty() {
		print_none_row();
		return;
	}
	for row in rows {
		println!(
			"  {fp}  {addr}  sni={sni} alpn=[{alpn}]",
			fp = row.fingerprint_id,
			addr = row.remote_addr,
			sni = row.sni,
			alpn = row.alpn.join(","),
		);
	}
}

fn print_connection_rows(rows: &[ConnectionInfo]) {
	if rows.is_empty() {
		print_none_row();
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
