use bevy::prelude::*;

use crate::playback::PlayerSoundPlugin;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        crate::backend::register(app);
        app.add_plugins(PlayerSoundPlugin);
        crate::match_set::register(app);
        crate::frontend::register_frontend_audio(app);
        crate::entity_events::register_entity_event_audio(app);
        crate::rumble::register(app);
        crate::script_music::register(app);
        crate::script_mix::register(app);
        // wasm has no control thread: one pass per frame on the main thread.
        #[cfg(target_family = "wasm")]
        app.add_systems(PostUpdate, wasm_control_system);
    }
}

#[cfg(target_family = "wasm")]
fn wasm_control_system() {
    crate::runtime::wasm_control_pass();
}
