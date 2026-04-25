use std::sync::atomic::{AtomicU8, Ordering};

use vane_core::FlowLogVerbosity;

/// Daemon-global flow-log verbosity selector. Listeners read `current()`
/// once per accepted connection to populate `FlowCtx::verbosity`; the
/// management API toggles via `set(..)` (S1-29 verb).
///
/// Stored as `AtomicU8` so reads are lock-free and writes are uncontended.
/// In-flight connections retain whatever verbosity they were built with;
/// `set(..)` only affects subsequent `current()` reads.
pub struct VerbosityState {
	level: AtomicU8,
}

impl VerbosityState {
	#[must_use]
	pub const fn new() -> Self {
		Self { level: AtomicU8::new(0) }
	}

	#[must_use]
	pub fn current(&self) -> FlowLogVerbosity {
		match self.level.load(Ordering::Relaxed) {
			1 => FlowLogVerbosity::Debug,
			_ => FlowLogVerbosity::Trajectory,
		}
	}

	pub fn set(&self, v: FlowLogVerbosity) {
		let n = match v {
			FlowLogVerbosity::Trajectory => 0,
			FlowLogVerbosity::Debug => 1,
		};
		self.level.store(n, Ordering::Relaxed);
	}
}

impl Default for VerbosityState {
	fn default() -> Self {
		Self::new()
	}
}
