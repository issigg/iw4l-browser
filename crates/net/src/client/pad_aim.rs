use super::input::ClientActionInput;

const TURN_YAW: f32 = 260.0;
const TURN_PITCH: f32 = 90.0;
const TURN_YAW_ADS: f32 = 90.0;
const TURN_PITCH_ADS: f32 = 55.0;
const TURN_ACCEL: f32 = 1200.0;

const SLOWDOWN_HIP: f32 = 0.4;
const SLOWDOWN_ADS: f32 = 0.5;
const SLOWDOWN_REGION: [f32; 2] = [90.0, 90.0];
const LOCKON_REGION: [f32; 2] = [90.0, 90.0];
const AUTOAIM_REGION: [f32; 2] = [160.0, 120.0];
const LOCKON_STRENGTH: f32 = 0.6;
const LOCKON_DEFLECTION: f32 = 0.05;
const AUTOAIM_LERP: f32 = 40.0;
const AUTOAIM_TIME: f32 = 0.5;

const FALLBACK_RANGE: f32 = 1500.0;

#[derive(Clone, Copy, Debug)]
pub struct AimTarget {
    pub key: u64,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub aim: [f32; 3],
    /// Mid-chest, used by the mouse assist.
    pub chest: [f32; 3],
    pub velocity: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct AimView {
    pub eye: [f32; 3],
    /// Pitch and yaw, degrees.
    pub angles: [f32; 2],
    pub velocity: [f32; 3],
    pub ads_lerp: f32,
    /// Against a 65 degree field of view.
    pub fov_scale: f32,
    pub ranges: weapon_iw4::AimAssistRanges,
    pub dt: f32,
    /// Fire button held.
    pub firing: bool,
    /// Weapon fire interval; long means one shot at a time (snipers), short means spraying.
    pub fire_time_ms: i32,
}

struct OnScreen {
    target: AimTarget,
    min: [f32; 2],
    max: [f32; 2],
    dist_sqr: f32,
    crosshair_sqr: f32,
}

fn axes(angles: [f32; 2]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [sy, -cy, 0.0];
    let up = [sp * cy, sp * sy, cp];
    (forward, right, up)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn angles_to(eye: [f32; 3], point: [f32; 3]) -> [f32; 2] {
    let d = [point[0] - eye[0], point[1] - eye[1], point[2] - eye[2]];
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    [
        -d[2].atan2(flat).to_degrees(),
        d[1].atan2(d[0]).to_degrees(),
    ]
}

fn angle_delta(to: f32, from: f32) -> f32 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn project(view: &AimView, targets: &[AimTarget]) -> Vec<OnScreen> {
    let (forward, right, up) = axes(view.angles);
    let tan_x = (32.5f32.to_radians().tan() * view.fov_scale).max(1e-3);
    let tan_y = tan_x * 9.0 / 16.0;
    let mut out: Vec<OnScreen> = targets
        .iter()
        .filter_map(|target| {
            let mut min = [f32::MAX; 2];
            let mut max = [f32::MIN; 2];
            for corner in 0..8 {
                let p = [
                    if corner & 1 == 0 {
                        target.mins[0]
                    } else {
                        target.maxs[0]
                    },
                    if corner & 2 == 0 {
                        target.mins[1]
                    } else {
                        target.maxs[1]
                    },
                    if corner & 4 == 0 {
                        target.mins[2]
                    } else {
                        target.maxs[2]
                    },
                ];
                let d = [p[0] - view.eye[0], p[1] - view.eye[1], p[2] - view.eye[2]];
                let depth = dot(d, forward);
                if depth <= 0.0 {
                    continue;
                }
                let x = dot(d, right) / (depth * tan_x);
                let y = -dot(d, up) / (depth * tan_y);
                min = [min[0].min(x), min[1].min(y)];
                max = [max[0].max(x), max[1].max(y)];
            }
            if max[0] <= min[0]
                || max[1] <= min[1]
                || min[0] > 1.0
                || min[1] > 1.0
                || max[0] < -1.0
                || max[1] < -1.0
            {
                return None;
            }
            let min = [min[0].clamp(-1.0, 1.0), min[1].clamp(-1.0, 1.0)];
            let max = [max[0].clamp(-1.0, 1.0), max[1].clamp(-1.0, 1.0)];
            let centre = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
            let d = [
                target.aim[0] - view.eye[0],
                target.aim[1] - view.eye[1],
                target.aim[2] - view.eye[2],
            ];
            Some(OnScreen {
                target: *target,
                min,
                max,
                dist_sqr: dot(d, d),
                crosshair_sqr: centre[0] * centre[0] + centre[1] * centre[1],
            })
        })
        .collect();
    out.sort_by(|a, b| a.crosshair_sqr.total_cmp(&b.crosshair_sqr));
    out
}

fn best(screen: &[OnScreen], range: f32, region: [f32; 2], scale: f32) -> Option<&OnScreen> {
    let half = [region[0] / 640.0 * scale, region[1] / 480.0 * scale];
    screen.iter().find(|s| {
        s.dist_sqr <= range * range
            && s.min[0] <= half[0]
            && s.max[0] >= -half[0]
            && s.min[1] <= half[1]
            && s.max[1] >= -half[1]
    })
}

pub fn pad_look_frame(
    input: &mut ClientActionInput,
    view: &AimView,
    targets: &[AimTarget],
    ads: bool,
) {
    let dt = view.dt;
    let ads_lerp = view.ads_lerp.clamp(0.0, 1.0);
    let sensitivity = input.pad_sensitivity * lerp(1.0, input.pad_ads_sensitivity, ads_lerp);
    let zoom = view.fov_scale.clamp(0.02, 2.0);
    let max_yaw = lerp(TURN_YAW, TURN_YAW_ADS, ads_lerp) * zoom * sensitivity;
    let max_pitch = lerp(TURN_PITCH, TURN_PITCH_ADS, ads_lerp) * zoom * sensitivity;
    let wanted = [-input.pad_look[1] * max_pitch, -input.pad_look[0] * max_yaw];

    for (rate, goal) in input.pad_turn_rate.iter_mut().zip(wanted) {
        if input.pad_acceleration && goal.abs() > rate.abs() {
            let step = TURN_ACCEL * input.pad_sensitivity * dt;
            *rate = (rate.abs() + step).min(goal.abs()) * goal.signum();
        } else {
            *rate = goal;
        }
    }
    let mut delta = [input.pad_turn_rate[0] * dt, input.pad_turn_rate[1] * dt];

    let mode = input.pad_aim_assist;
    if mode == 0 {
        input.pad_lockon = None;
        input.pad_autoaim = None;
        input.pad_was_ads = ads;
        input.pad_look_delta = delta;
        return;
    }
    let screen = project(view, targets);
    let range = |hip: f32, ads: f32| {
        let hip = if hip > 0.0 { hip } else { FALLBACK_RANGE };
        let ads = if ads > 0.0 { ads } else { hip };
        lerp(hip, ads, ads_lerp)
    };
    let assist_range = range(view.ranges.hip, view.ranges.ads);

    if best(&screen, assist_range, SLOWDOWN_REGION, 1.0).is_some() {
        let slow = lerp(SLOWDOWN_HIP, SLOWDOWN_ADS, ads_lerp);
        delta[0] *= slow;
        delta[1] *= slow;
    }

    let kept = input
        .pad_lockon
        .and_then(|key| screen.iter().find(|s| s.target.key == key))
        .filter(|s| best(std::slice::from_ref(*s), assist_range, LOCKON_REGION, 1.0).is_some());
    let lock = kept.or_else(|| best(&screen, assist_range, LOCKON_REGION, 1.0));
    input.pad_lockon = lock.map(|s| s.target.key);
    if let Some(lock) = lock
        && input.pad_autoaim.is_none()
        && input.pad_deflection > LOCKON_DEFLECTION
    {
        let t = lock.target;
        let moved = [
            t.aim[0] + (t.velocity[0] - view.velocity[0]) * dt,
            t.aim[1] + (t.velocity[1] - view.velocity[1]) * dt,
            t.aim[2] + (t.velocity[2] - view.velocity[2]) * dt,
        ];
        let now = angles_to(view.eye, t.aim);
        let next = angles_to(view.eye, moved);
        delta[0] += LOCKON_STRENGTH * angle_delta(next[0], now[0]);
        delta[1] += LOCKON_STRENGTH * angle_delta(next[1], now[1]);
    }

    if mode == 2 && ads && !input.pad_was_ads {
        let auto_range = if view.ranges.auto_aim > 0.0 {
            view.ranges.auto_aim
        } else {
            FALLBACK_RANGE
        };
        input.pad_autoaim = best(&screen, auto_range, AUTOAIM_REGION, (1.0 / zoom).max(1.0))
            .map(|s| (s.target.key, AUTOAIM_TIME));
    }
    if !ads {
        input.pad_autoaim = None;
    }
    if let Some((key, left)) = input.pad_autoaim {
        match screen
            .iter()
            .find(|s| s.target.key == key)
            .filter(|_| left > 0.0)
        {
            Some(s) => {
                let goal = angles_to(view.eye, s.target.aim);
                let pitch = angle_delta(goal[0], view.angles[0]);
                let yaw = angle_delta(goal[1], view.angles[1]);
                let length = pitch.hypot(yaw);
                let step = (AUTOAIM_LERP * dt).min(length);
                if length > 1e-4 {
                    delta[0] += pitch / length * step;
                    delta[1] += yaw / length * step;
                }
                input.pad_autoaim = (length > 0.25).then_some((key, left - dt));
            }
            None => input.pad_autoaim = None,
        }
    }
    input.pad_was_ads = ads;
    input.pad_look_delta = delta;
}

const MOUSE_SLOW_LOW: [f32; 2] = [0.65, 0.5];
const MOUSE_SLOW_HIGH: [f32; 2] = [0.45, 0.3];
/// Level 3 ("unfair"): the mouse nearly sticks to the target.
const MOUSE_SLOW_INSANE: [f32; 2] = [0.45, 0.3];
const MOUSE_REGION: [f32; 2] = [130.0, 130.0];
const MOUSE_PULL_REGION: [f32; 2] = [100.0, 100.0];
/// Fraction of the mouse's own motion added toward the target.
const MOUSE_PULL_FRACTION: f32 = 0.35;
/// Never close more than this share of the remaining error in one frame.
const MOUSE_PULL_ERROR_SHARE: f32 = 0.35;
/// Hard cap, degrees per frame.
const MOUSE_PULL_MAX: f32 = 1.0;
/// Below this alignment (cosine) between motion and target direction there is no pull.
const MOUSE_PULL_MIN_ALIGN: f32 = 0.3;

/// Mouse delta scale while a hostile is inside the assist region.
pub fn mouse_slowdown(mode: u8, ads_lerp: f32) -> f32 {
    let pair = match mode {
        0 => return 1.0,
        1 => MOUSE_SLOW_LOW,
        2 => MOUSE_SLOW_HIGH,
        _ => MOUSE_SLOW_INSANE,
    };
    lerp(pair[0], pair[1], ads_lerp.clamp(0.0, 1.0))
}

/// Pull magnitude in degrees for this frame. `motion` is the angular size of the
/// player's own mouse movement, `error` the angle to the target and `align` the
/// cosine between the movement and the direction to the target.
pub fn mouse_pull_amount(motion: f32, error: f32, align: f32) -> f32 {
    if motion <= 0.0 || error <= 0.0 || align < MOUSE_PULL_MIN_ALIGN {
        return 0.0;
    }
    (motion * MOUSE_PULL_FRACTION * align)
        .min(error * MOUSE_PULL_ERROR_SHARE)
        .min(MOUSE_PULL_MAX)
}

/// Mouse counterpart of `pad_look_frame`: scales the raw mouse deltas down near
/// a target and, at level 2, adds a small pull while moving toward it.
pub fn mouse_look_frame(input: &mut ClientActionInput, view: &AimView, targets: &[AimTarget]) {
    input.mouse_pull = [0.0; 2];
    let mode = input.mouse_aim_assist;
    if mode >= 3 {
        mouse_look_unfair(input, view, targets);
        return;
    }
    if mode == 0 || (input.mouse_x == 0.0 && input.mouse_y == 0.0) {
        return;
    }
    let ads_lerp = view.ads_lerp.clamp(0.0, 1.0);
    let screen = project(view, targets);
    let hip = if view.ranges.hip > 0.0 {
        view.ranges.hip
    } else {
        FALLBACK_RANGE
    };
    let ads = if view.ranges.ads > 0.0 {
        view.ranges.ads
    } else {
        hip
    };
    let range = lerp(hip, ads, ads_lerp);
    if best(&screen, range, MOUSE_REGION, 1.0).is_none() {
        return;
    }
    let slow = mouse_slowdown(mode, ads_lerp);
    // Angular intent before the slowdown: pitch follows +y, yaw follows -x.
    let sens = (input.sensitivity * view.fov_scale).max(0.0);
    let intent = [
        input.mouse_y * input.m_pitch * sens,
        -input.mouse_x * input.m_yaw * sens,
    ];
    input.mouse_x *= slow;
    input.mouse_y *= slow;
    if mode < 2 {
        return;
    }
    let Some(target) = best(&screen, range, MOUSE_PULL_REGION, 1.0) else {
        return;
    };
    let goal = angles_to(view.eye, target.target.aim);
    let err = [
        angle_delta(goal[0], view.angles[0]),
        angle_delta(goal[1], view.angles[1]),
    ];
    let error = err[0].hypot(err[1]);
    let motion = intent[0].hypot(intent[1]) * slow;
    if error <= 1e-4 || motion <= 1e-6 {
        return;
    }
    let align = (intent[0] * err[0] + intent[1] * err[1]) / (intent[0].hypot(intent[1]) * error);
    let pull = mouse_pull_amount(motion, error, align);
    input.mouse_pull = [err[0] / error * pull, err[1] / error * pull];
}

/// Angular error (degrees) inside which the unfair mode grabs a target.
const UNFAIR_CONE: f32 = 30.0;
/// Inside this error the mouse is slowed down.
/// Half the width of a player body, units; sets how big the on-target zone is at a distance.
const UNFAIR_BODY_RADIUS: f32 = 18.0;

/// Shots closer together than this count as spraying (sustained boost while held); slower
/// weapons get a short boost right after each shot.
const SPRAY_MAX_FIRE_TIME_MS: i32 = 250;
/// How long the one-shot boost lasts, seconds.
const SHOT_BOOST_SECONDS: f32 = 0.3;

/// Smooth 0..1 ramp.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A mouse move this big (degrees per frame) away from the target breaks a shot lock.
const UNFAIR_BREAKAWAY: f32 = 2.5;

/// Level 3. Normally it only slows the mouse a little near an enemy (no pull, so moving away
/// is easy). When you shoot it relocks onto the enemy's chest and cancels most of your own
/// mouse movement: a short lock per shot for slow weapons, held while spraying for fast ones.
fn mouse_look_unfair(input: &mut ClientActionInput, view: &AimView, targets: &[AimTarget]) {
    let attack_was = std::mem::replace(&mut input.mouse_attack_prev, view.firing);
    let spray = view.fire_time_ms < SPRAY_MAX_FIRE_TIME_MS;
    if view.firing && (spray || !attack_was) {
        input.mouse_shot_boost = 1.0;
    } else if !(view.firing && spray) {
        input.mouse_shot_boost = (input.mouse_shot_boost - view.dt / SHOT_BOOST_SECONDS).max(0.0);
    }
    let lock = input.mouse_shot_boost;
    let moving = input.mouse_x != 0.0 || input.mouse_y != 0.0;
    if !moving && lock <= 0.0 {
        return;
    }
    let mut best: Option<([f32; 2], f32, f32)> = None;
    for target in targets {
        let d = [
            target.chest[0] - view.eye[0],
            target.chest[1] - view.eye[1],
            target.chest[2] - view.eye[2],
        ];
        let dist = dot(d, d).sqrt();
        if dist > 4000.0 {
            continue;
        }
        let goal = angles_to(view.eye, target.chest);
        let err = [
            angle_delta(goal[0], view.angles[0]),
            angle_delta(goal[1], view.angles[1]),
        ];
        let error = err[0].hypot(err[1]);
        if error <= UNFAIR_CONE && best.is_none_or(|(_, e, _)| error < e) {
            best = Some((err, error, dist));
        }
    }
    let Some((err, error, dist)) = best else {
        return;
    };
    let ads_lerp = view.ads_lerp.clamp(0.0, 1.0);
    let reach = smooth((UNFAIR_CONE - error) / 8.0);
    let sens = (input.sensitivity * view.fov_scale).max(0.0);
    let intent = [
        input.mouse_y * input.m_pitch * sens,
        -input.mouse_x * input.m_yaw * sens,
    ];
    let intent_len = intent[0].hypot(intent[1]);
    let align = if intent_len > 1e-6 && error > 1e-3 {
        (intent[0] * err[0] + intent[1] * err[1]) / (intent_len * error)
    } else {
        0.0
    };
    if lock <= 0.0 {
        // Not shooting: slow the mouse near the enemy, easing off when moving away.
        // Only while the crosshair is on the enemy: the body's angular size at this distance.
        let on_body = (UNFAIR_BODY_RADIUS / dist.max(1.0)).atan().to_degrees().clamp(1.5, 6.0);
        if error <= on_body {
            let hold = smooth(1.0 + align * 1.5);
            let edge = smooth((on_body - error) / (on_body * 0.5));
            let slow = lerp(1.0, mouse_slowdown(3, ads_lerp), hold * edge);
            input.mouse_x *= slow;
            input.mouse_y *= slow;
        }
        return;
    }
    // A hard flick away from the enemy breaks the lock.
    if intent_len > UNFAIR_BREAKAWAY && align < -0.2 {
        input.mouse_shot_boost = 0.0;
        return;
    }
    // Cancel most of the player's own movement and close most of the remaining error.
    let keep = 1.0 - 0.9 * lock * reach;
    input.mouse_x *= keep;
    input.mouse_y *= keep;
    if error > 1e-3 {
        let pull = (error * 0.6 * lock * reach).min(6.0);
        input.mouse_pull = [err[0] / error * pull, err[1] / error * pull];
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    #[test]
    fn slowdown_scales() {
        assert_eq!(mouse_slowdown(0, 1.0), 1.0);
        assert_eq!(mouse_slowdown(1, 0.0), 0.65);
        assert_eq!(mouse_slowdown(2, 0.0), 0.45);
        assert!(mouse_slowdown(2, 1.0) < mouse_slowdown(2, 0.0));
        assert!(mouse_slowdown(1, 1.0) > mouse_slowdown(2, 1.0));
        assert!(mouse_slowdown(3, 0.0) <= mouse_slowdown(2, 0.0));
    }

    #[test]
    fn pull_is_limited_and_directional() {
        assert_eq!(mouse_pull_amount(0.0, 5.0, 1.0), 0.0);
        assert_eq!(mouse_pull_amount(2.0, 5.0, -0.5), 0.0);
        assert_eq!(mouse_pull_amount(2.0, 5.0, 0.1), 0.0);
        assert!(mouse_pull_amount(100.0, 100.0, 1.0) <= MOUSE_PULL_MAX);
        assert!(mouse_pull_amount(100.0, 0.5, 1.0) <= 0.5 * MOUSE_PULL_ERROR_SHARE + 1e-6);
        assert!(mouse_pull_amount(1.0, 5.0, 1.0) > 0.0);
    }
}
