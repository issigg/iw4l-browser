//! WASM-SPIKE: a Bevy window backend for the engine running in a dedicated worker.
//!
//! `bevy_winit` needs the DOM, which a worker does not have. This plugin replaces it: the window
//! is an `OffscreenCanvas` the page transferred, the frame loop is the worker's
//! `requestAnimationFrame`, and input arrives as messages from the page (see
//! `crates/web/www/PROTOCOL.md`).

#[cfg(target_family = "wasm")]
mod imp;
#[cfg(target_family = "wasm")]
pub use imp::*;
