//! Interactive terminal UI bound to bare `vane` (and the explicit
//! `vane tui` form). The module currently owns only the lifecycle
//! scaffold: enter alt screen + raw mode, render a placeholder frame,
//! poll events, restore the terminal on every exit path including
//! panic.
//!
//! See [`spec/tui.md`](../../../spec/tui.md) for the design.
//
// TODO(tui-views): land the view state machine, management-client
// wiring, and the per-view widgets (Connections / Flow log /
// Structured log / Certs / Metrics / Config / Pools).

use std::io::{self, Stdout};
use std::panic;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
	EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use vane_core::version::BuildInfo;

type Tty = Terminal<CrosstermBackend<Stdout>>;

/// Run the TUI to completion. Set up the terminal, drive the event
/// loop, then restore the terminal regardless of how the loop exits
/// (clean quit, error, panic). Returns whatever the event loop
/// surfaced.
pub(crate) fn run(info: &BuildInfo) -> anyhow::Result<()> {
	install_panic_hook();
	let mut terminal = enter()?;
	let result = event_loop(&mut terminal, info);
	leave(&mut terminal);
	result
}

fn enter() -> anyhow::Result<Tty> {
	enable_raw_mode().context("enable raw mode")?;
	let mut stdout = io::stdout();
	execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
	Terminal::new(CrosstermBackend::new(stdout)).context("construct ratatui terminal")
}

/// Best-effort terminal restore. Each step is independently fallible
/// — we still want to attempt the rest even if one fails so the user
/// doesn't end up with a half-restored terminal after an error path.
fn leave(terminal: &mut Tty) {
	let _ = disable_raw_mode();
	let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
	let _ = terminal.show_cursor();
}

/// Install a process-wide panic hook that restores the terminal
/// before delegating to the previous hook (typically the default
/// stderr backtrace printer). Without this, a panic mid-render
/// leaves the user staring at a wrecked TTY (no echo, no line
/// discipline).
fn install_panic_hook() {
	let prev = panic::take_hook();
	panic::set_hook(Box::new(move |info| {
		let _ = disable_raw_mode();
		let _ = execute!(io::stdout(), LeaveAlternateScreen);
		prev(info);
	}));
}

fn event_loop(terminal: &mut Tty, info: &BuildInfo) -> anyhow::Result<()> {
	// 250 ms poll cadence: tight enough that a quit keypress feels
	// instantaneous, slow enough that the loop spends most of its time
	// parked instead of redrawing. Once the management client is
	// wired in, the cadence will be driven by per-view refresh
	// intervals (see spec/tui.md § _Update
	// model_) rather than a fixed timer.
	let tick = Duration::from_millis(250);
	loop {
		terminal.draw(|f| draw(f, info))?;
		if event::poll(tick)?
			&& let Event::Key(key) = event::read()?
			&& key.kind == KeyEventKind::Press
			&& should_quit(key.code, key.modifiers)
		{
			return Ok(());
		}
	}
}

fn should_quit(code: KeyCode, mods: KeyModifiers) -> bool {
	match code {
		KeyCode::Char('q' | 'Q') | KeyCode::Esc => true,
		KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => true,
		_ => false,
	}
}

fn draw(f: &mut ratatui::Frame, info: &BuildInfo) {
	let layout = Layout::default()
		.direction(Direction::Vertical)
		.constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
		.split(f.area());

	let header = Paragraph::new(Line::from(vec![
		Span::styled("Vane", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
		Span::raw(format!("  {}  ({})", info.version, info.commit)),
	]))
	.block(Block::default().borders(Borders::ALL));
	f.render_widget(header, layout[0]);

	let body = Paragraph::new(vec![
		Line::from(""),
		Line::from("  TUI scaffold — views not yet implemented."),
		Line::from(""),
		Line::from(vec![
			Span::raw("  No daemon connection yet; "),
			Span::styled("press q to quit", Style::default().fg(Color::Cyan)),
			Span::raw("."),
		]),
	]);
	f.render_widget(body, layout[1]);

	let footer = Paragraph::new(Line::from(vec![
		Span::styled(" q ", Style::default().bg(Color::Cyan).fg(Color::Black)),
		Span::raw(" quit  "),
		Span::styled(" Esc ", Style::default().bg(Color::Cyan).fg(Color::Black)),
		Span::raw(" quit  "),
		Span::styled(" Ctrl-C ", Style::default().bg(Color::Cyan).fg(Color::Black)),
		Span::raw(" quit"),
	]));
	f.render_widget(footer, layout[2]);
}
