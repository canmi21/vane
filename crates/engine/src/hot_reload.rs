//! File watcher + debounce (via `notify` + `notify-debouncer-full`) and
//! `ArcSwap<FlowGraph>` store (skip when `version_hash` unchanged).
//!
//! Boot-time compile is driven explicitly by the boot sequence — not by
//! the watcher; existing files fire no `notify` event on boot (see
//! `spec/roadmap.md` § _Bootstrapping problems_).
//!
//! See `spec/architecture/02-flow.md` § _Hot reload_ and
//! `spec/architecture/09-config.md` § _Reload_. Features: S1-27, S1-28.
