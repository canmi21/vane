//! Preset expansion stage — `PresetInvocation → Vec<RawRule>`. Emits
//! raw rules with **string** middleware references; resolution to
//! concrete kinds happens during `lower`.
//!
//! See `spec/architecture/14-presets.md`.
