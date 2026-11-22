//! The Linux Store GUI as a library, so `main.rs` and the examples share the same wiring.
//! `convert` is pure (engine types to UI data); `wiring` connects the window to the engine.

slint::include_modules!();

pub mod convert;
pub mod wiring;
