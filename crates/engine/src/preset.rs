//! Preset expansion implementations registered against core's `expand`
//! stage. Stage 1 catalog: `port_forward`, `reverse_proxy` (without the
//! WS gate — WebSocket lands in Stage 2), `static_site`, `redirect_https`.
//!
//! See `spec/architecture/14-presets.md`. Feature: S1-22.
