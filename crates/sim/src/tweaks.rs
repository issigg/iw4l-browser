//! Host "game tweaks" (`scr_tweak_*` rule dvars). Parsed once per match and
//! stored as a resource on the sim world; every field defaults to stock MW2.

use bevy_ecs::prelude::Resource;

use crate::frame::FrameWorld;
use crate::world::ClientId;

/// The listen-server's local human is always client 0; bots get other ids.
pub const LOCAL_TWEAK_CLIENT: ClientId = ClientId(0);

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tweaks {
    pub onehit: bool,
    pub infinite_ammo: bool,
    pub speed_pct: u32,
    pub jump_pct: u32,
    pub gravity_pct: u32,
    pub timescale_pct: u32,
    pub recoil_pct: u32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Self {
            onehit: false,
            infinite_ammo: false,
            speed_pct: 100,
            jump_pct: 100,
            gravity_pct: 100,
            timescale_pct: 100,
            recoil_pct: 100,
        }
    }
}

impl Tweaks {
    pub fn from_rules<'a>(rules: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut tweaks = Self::default();
        for (name, value) in rules {
            let Ok(n) = value.trim().parse::<f64>() else {
                continue;
            };
            let n = n.round().clamp(0.0, 100_000.0) as u32;
            match name.to_ascii_lowercase().as_str() {
                "scr_tweak_onehit" => tweaks.onehit = n != 0,
                "scr_tweak_infiniteammo" => tweaks.infinite_ammo = n != 0,
                "scr_tweak_speed" => tweaks.speed_pct = n.clamp(25, 400),
                "scr_tweak_jump" => tweaks.jump_pct = n.clamp(25, 500),
                "scr_tweak_gravity" => tweaks.gravity_pct = n.clamp(10, 400),
                "scr_tweak_timescale" => tweaks.timescale_pct = n.clamp(25, 200),
                "scr_tweak_recoil" => tweaks.recoil_pct = n.min(100),
                _ => {}
            }
        }
        tweaks
    }

    pub fn time_scale(&self) -> f64 {
        f64::from(self.timescale_pct) / 100.0
    }

    /// Damage that makes a hit lethal when `onehit` applies to this attacker.
    pub fn one_hit_amount(&self, attacker: ClientId, target: ClientId, amount: i32) -> i32 {
        if self.onehit && attacker == LOCAL_TWEAK_CLIENT && target != attacker {
            amount.max(100_000)
        } else {
            amount
        }
    }
}

pub(crate) fn current(world: &mut FrameWorld) -> Tweaks {
    world
        .ecs()
        .get_resource::<Tweaks>()
        .copied()
        .unwrap_or_default()
}

/// Top up the held weapon of `id` (clip and reserve) after a command.
pub(crate) fn refill_held_ammo(world: &mut FrameWorld, id: ClientId) {
    let Some(weapon) = world.player(id).map(|ps| ps.weapon) else {
        return;
    };
    if weapon == 0 {
        return;
    }
    let Some((clip_size, max_ammo, ammo_index, clip_index, dual)) =
        world.combat_facts_for(weapon).map(|f| {
            (
                f.clip_size,
                f.max_ammo,
                weapon_iw4::ammo_table_key(f.ammo_index, weapon),
                weapon_iw4::clip_table_key(f.clip_index, weapon),
                f.dual_wield,
            )
        })
    else {
        return;
    };
    if clip_size <= 0 {
        return;
    }
    let stock = (max_ammo - clip_size).max(0);
    if let Some(ps) = world.player_mut(id) {
        if ammo_index != 0 {
            let _ = weapon_iw4::set_ammo_not_in_clip(&mut ps.ammo, ammo_index, stock);
        }
        if clip_index != 0 {
            let _ = weapon_iw4::set_clip_for_hand(&mut ps.ammoclip, clip_index, 0, clip_size);
            if dual {
                let _ = weapon_iw4::set_clip_for_hand(&mut ps.ammoclip, clip_index, 1, clip_size);
            }
        }
    }
    world.client_meta_mut(id).set_ammo(weapon, clip_size, stock);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_clamps() {
        let t = Tweaks::from_rules([
            ("scr_tweak_onehit", "1"),
            ("scr_tweak_speed", "1000"),
            ("scr_tweak_jump", "1"),
            ("scr_tweak_gravity", "5"),
            ("scr_tweak_timescale", "500"),
            ("scr_tweak_infiniteammo", "0"),
            ("scr_other", "7"),
        ]);
        assert!(t.onehit && !t.infinite_ammo);
        assert_eq!((t.speed_pct, t.jump_pct, t.gravity_pct, t.timescale_pct), (400, 25, 10, 200));
        assert_eq!(Tweaks::from_rules([]), Tweaks::default());
    }

    #[test]
    fn one_hit_only_for_local_attacker() {
        let t = Tweaks { onehit: true, ..Tweaks::default() };
        assert!(t.one_hit_amount(ClientId(0), ClientId(3), 20) >= 100_000);
        assert_eq!(t.one_hit_amount(ClientId(2), ClientId(0), 20), 20);
        assert_eq!(t.one_hit_amount(ClientId(0), ClientId(0), 20), 20);
        assert_eq!(Tweaks::default().one_hit_amount(ClientId(0), ClientId(3), 20), 20);
    }
}
