//! "Game tweaks": cycle-able match options shown on the Private Match > Game Tweaks screen.
//!
//! Each tweak is a dvar plus an ordered list of (value, label). Clicking a row advances to the
//! next value. Rule dvars (`scr_*`, `g_hardcore`) travel to the match through
//! `HostMatchRules`; `ui_pad_aim_assist` / `ui_mouse_aim_assist` are mirrored into the user
//! settings. On the web the chosen values are also kept in `/artifacts/tweaks.cfg`.
use frame::UiMenuDvars;

pub(crate) struct Tweak {
    pub id: &'static str,
    pub label: &'static str,
    pub dvar: &'static str,
    /// (dvar value, shown text); the first entry is the default.
    pub values: &'static [(&'static str, &'static str)],
}

pub(crate) const TWEAKS: &[Tweak] = &[
    Tweak {
        id: "onehit",
        label: "ONE-HIT KILLS (YOU)",
        dvar: "scr_tweak_onehit",
        values: &[("0", "OFF"), ("1", "ON")],
    },
    Tweak {
        id: "health",
        label: "PLAYER HEALTH",
        dvar: "scr_player_maxhealth",
        values: &[("100", "NORMAL"), ("30", "LOW"), ("200", "DOUBLE"), ("500", "TANK")],
    },
    Tweak {
        id: "hardcore",
        label: "HARDCORE MODE",
        dvar: "g_hardcore",
        values: &[("0", "OFF"), ("1", "ON")],
    },
    Tweak {
        id: "speed",
        label: "RUN SPEED",
        dvar: "scr_tweak_speed",
        values: &[("100", "NORMAL"), ("125", "FAST"), ("150", "FASTER"), ("200", "SUPER")],
    },
    Tweak {
        id: "jump",
        label: "JUMP HEIGHT",
        dvar: "scr_tweak_jump",
        values: &[("100", "NORMAL"), ("150", "HIGH"), ("200", "HIGHER"), ("350", "SUPER")],
    },
    Tweak {
        id: "gravity",
        label: "GRAVITY",
        dvar: "scr_tweak_gravity",
        values: &[("100", "NORMAL"), ("50", "LOW"), ("25", "MOON")],
    },
    Tweak {
        id: "timescale",
        label: "GAME SPEED",
        dvar: "scr_tweak_timescale",
        values: &[("100", "NORMAL"), ("75", "SLOWER"), ("50", "SLOW MOTION"), ("150", "FAST")],
    },
    Tweak {
        id: "ammo",
        label: "INFINITE AMMO",
        dvar: "scr_tweak_infiniteammo",
        values: &[("0", "OFF"), ("1", "ON")],
    },
    Tweak {
        id: "killstreaks",
        label: "KILLSTREAKS",
        dvar: "scr_game_hardpoints",
        values: &[("1", "ON"), ("0", "OFF")],
    },
    Tweak {
        id: "recoil",
        label: "WEAPON RECOIL",
        dvar: "scr_tweak_recoil",
        values: &[("100", "NORMAL"), ("50", "HALF"), ("25", "LOW"), ("0", "NONE")],
    },
    Tweak {
        id: "uav",
        label: "PERMANENT UAV",
        dvar: "scr_game_constantradar",
        values: &[("0", "OFF"), ("1", "ON")],
    },
    Tweak {
        id: "padassist",
        label: "CONTROLLER AIM ASSIST",
        dvar: "ui_pad_aim_assist",
        values: &[("0", "OFF"), ("1", "ON")],
    },
    Tweak {
        id: "mouseassist",
        label: "MOUSE AIM ASSIST",
        dvar: "ui_mouse_aim_assist",
        values: &[("0", "OFF"), ("1", "LOW"), ("2", "HIGH"), ("3", "UNFAIR")],
    },
];

const SAVE_PATH: &str = "/artifacts/tweaks.cfg";

fn current_index(tweak: &Tweak, dvars: &UiMenuDvars) -> usize {
    let value = dvars.get(tweak.dvar).map(str::trim);
    tweak
        .values
        .iter()
        .position(|(v, _)| Some(*v) == value)
        .unwrap_or(0)
}

/// Dvar that holds the row text, e.g. `ui_tweak_speed` = "RUN SPEED: FAST".
pub(crate) fn label_dvar(tweak: &Tweak) -> String {
    format!("ui_tweak_{}", tweak.id)
}

/// Refresh every row's text from the current dvar values.
pub(crate) fn refresh_labels(dvars: &mut UiMenuDvars) {
    for tweak in TWEAKS {
        let shown = tweak.values[current_index(tweak, dvars)].1;
        dvars.set(&label_dvar(tweak), format!("{}: {}", tweak.label, shown));
    }
}

/// Copy the aim-assist rows into the user settings, which is what input reads.
pub(crate) fn sync_settings(dvars: &UiMenuDvars, settings: &mut frame::GameSettings) {
    let read = |name: &str| dvars.get(name).and_then(|v| v.trim().parse::<u8>().ok());
    let (pad, mouse) = (read("ui_pad_aim_assist"), read("ui_mouse_aim_assist"));
    if pad.is_some_and(|v| v != settings.pad_aim_assist) || mouse.is_some_and(|v| v != settings.mouse_aim_assist) {
        settings.pad_aim_assist = pad.unwrap_or(settings.pad_aim_assist);
        settings.mouse_aim_assist = mouse.unwrap_or(settings.mouse_aim_assist);
        settings.sanitize();
        settings.touch();
    }
}

/// Advance one tweak to its next value and remember the choice.
pub(crate) fn cycle(dvars: &mut UiMenuDvars, id: &str) -> Result<(), String> {
    let tweak = TWEAKS
        .iter()
        .find(|tweak| tweak.id == id)
        .ok_or_else(|| format!("unknown tweak `{id}`"))?;
    let next = (current_index(tweak, dvars) + 1) % tweak.values.len();
    dvars.set(tweak.dvar, tweak.values[next].0);
    save(dvars);
    Ok(())
}

/// Apply saved choices once at startup (web only; no-op elsewhere or when nothing is saved).
pub(crate) fn load_saved(dvars: &mut UiMenuDvars) {
    if !cfg!(target_family = "wasm") {
        return;
    }
    let Ok(text) = vfs::fs::read_to_string(SAVE_PATH) else {
        return;
    };
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if let Some(tweak) = TWEAKS
            .iter()
            .find(|tweak| tweak.dvar == name.trim() && !tweak.dvar.starts_with("ui_"))
            && tweak.values.iter().any(|(v, _)| *v == value.trim())
        {
            dvars.set(tweak.dvar, value.trim());
        }
    }
}

fn save(dvars: &UiMenuDvars) {
    if !cfg!(target_family = "wasm") {
        return;
    }
    let mut text = String::new();
    for tweak in TWEAKS.iter().filter(|tweak| !tweak.dvar.starts_with("ui_")) {
        let value = tweak.values[current_index(tweak, dvars)].0;
        text.push_str(&format!("{}={}\n", tweak.dvar, value));
    }
    let _ = vfs::fs::write(SAVE_PATH, text);
}
