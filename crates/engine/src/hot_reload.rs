//! File watcher + debounce (via `notify` + `notify-debouncer-full`) and
//! `ArcSwap<FlowGraph>` store (skip when `version_hash` unchanged).
//!
//! Boot-time compile is driven explicitly by the boot sequence — not by
//! the watcher; existing files fire no `notify` event on boot (see
//! `spec/crates/core.md` § _Compile pipeline_).
//!
//! See `spec/crates/engine.md` § _Hot reload_ and
//! `spec/crates/engine.md` § _Hot reload_.
