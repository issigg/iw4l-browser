//! WASM-SPIKE: show the "about iw4l-browser" screen once, the first time the main menu appears.
//! The "seen" marker lives under /artifacts, which the vfs persists in the browser.
use bevy::prelude::*;
use frame::UiMenuRequest;

const SEEN: &str = "/artifacts/about_seen";
/// Frames to wait after the main menu appears so the menu stack is up before the popup opens.
const SETTLE_FRAMES: u32 = 90;

pub(crate) fn show_about_once(
    mut frames: Local<u32>,
    mut done: Local<bool>,
    screen: Res<frame::AppScreen>,
    world: Res<State<frame::MatchScope>>,
    mut menus: MessageWriter<UiMenuRequest>,
) {
    if !cfg!(target_family = "wasm") || *done {
        return;
    }
    if *screen != frame::AppScreen::MainMenu || *world.get() == frame::MatchScope::Live {
        return;
    }
    *frames += 1;
    if *frames < SETTLE_FRAMES {
        return;
    }
    *done = true;
    if vfs::fs::metadata(SEEN).is_ok() {
        return;
    }
    let _ = vfs::fs::write(SEEN, b"1");
    menus.write(UiMenuRequest::Open("iw4l_about".into()));
}
