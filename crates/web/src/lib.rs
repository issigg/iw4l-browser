// WASM-SPIKE: browser entry. Boots the menu against a (not yet existing) virtual games root.
use std::path::PathBuf;

use wasm_bindgen::prelude::*;

#[global_allocator]
static ALLOCATOR: diag::ProcessCountingAllocator = diag::ProcessCountingAllocator;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// `map`: when given (e.g. `mp_rust`), skip the menu and load that map straight away.
#[wasm_bindgen]
pub fn run(map: Option<String>) {
    let mode = match map {
        Some(zone) if !zone.is_empty() => bootstrap::LaunchMode::Map(zone),
        _ => bootstrap::LaunchMode::Menu,
    };
    bootstrap::launch(
        asset_transport::GamesRoot(PathBuf::from("/games")),
        PathBuf::from("/artifacts"),
        mode,
        None,
        sim::HostCheats::default(),
    );
}

/// Worker entry: the engine renders to the transferred `OffscreenCanvas` and takes input from
/// page messages (see `www/PROTOCOL.md`). Returns once the frame loop is scheduled.
#[wasm_bindgen]
pub fn worker_main(
    canvas: web_sys::OffscreenCanvas,
    map: Option<String>,
    width: u32,
    height: u32,
    scale: f32,
) {
    web_window::enable(canvas, width, height, scale);
    run(map);
}
