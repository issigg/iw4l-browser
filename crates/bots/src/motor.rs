use math_iw4::{angle_normalize_360, angle_subtract, yaw_vectors_2d};
use movement_iw4::ANGLE2SHORT;
use playerstate_iw4::{UserCmd, buttons};

use crate::intent::{BotIntent, MotorReport, MoveMode, PathOutcome};
use crate::observation::BotObservation;

const MAX_YAW_DEG_PER_MS: f32 = 0.36;
const MAX_PITCH_DEG_PER_MS: f32 = 0.24;
const MAX_YAW_ACCEL_DEG_PER_MS2: f32 = 0.015;
const MAX_PITCH_ACCEL_DEG_PER_MS2: f32 = 0.01;
const FIRE_CONE_DEG: f32 = 8.0;
const ARRIVE_IN: f32 = 24.0;
const PITCH_LIMIT: f32 = 70.0;
/// Bots only strafe while shooting at something closer than this (units).
const STRAFE_MAX_RANGE: f32 = 1200.0;
const STRAFE_SPEED: i8 = 64;
/// Side glances: how long one lasts (ms) and how far it turns the view (degrees).
const GLANCE_MS: f32 = 700.0;
const GLANCE_DEG: f32 = 35.0;
/// Closer than this (units) a bot holds the trigger; farther it fires in bursts.
const BURST_MIN_RANGE: f32 = 500.0;

#[derive(Clone, Debug)]
pub struct Motor {
    yaw: f32,
    pitch: f32,
    yaw_vel: f32,
    pitch_vel: f32,
    err_yaw: f32,
    err_pitch: f32,
    acquire_yaw: f32,
    last_look: Option<[f32; 3]>,
    armed: Option<sim::LifeSequence>,
    strafe_period_ms: i32,
    strafing: bool,
    /// Per-bot phases for the aim wobble, glance timing and burst rhythm.
    phase: [f32; 4],
    glance_period_ms: i32,
    burst_on_ms: i32,
    burst_off_ms: i32,
}

impl Motor {
    pub fn new(seed: u64) -> Self {
        let self_preserve = (((seed >> 8) & 0xff) as f32) / 255.0;
        let spread = 0.75 + 0.75 * self_preserve;
        let err_yaw = (((seed >> 3) & 0xffff) as f32 / 65535.0 - 0.5) * 4.0 * spread;
        let err_pitch = (((seed >> 11) & 0xffff) as f32 / 65535.0 - 0.5) * 2.0 * spread;
        Self {
            yaw: 0.0,
            pitch: 0.0,
            yaw_vel: 0.0,
            pitch_vel: 0.0,
            err_yaw,
            err_pitch,
            acquire_yaw: 0.0,
            last_look: None,
            armed: None,
            strafe_period_ms: 600 + ((seed >> 20) % 500) as i32,
            strafing: false,
            phase: [
                ((seed >> 5) & 0xff) as f32 / 255.0 * std::f32::consts::TAU,
                ((seed >> 13) & 0xff) as f32 / 255.0 * std::f32::consts::TAU,
                ((seed >> 21) & 0xff) as f32 / 255.0 * std::f32::consts::TAU,
                ((seed >> 29) & 0xff) as f32 / 255.0 * std::f32::consts::TAU,
            ],
            glance_period_ms: 2500 + ((seed >> 17) % 3000) as i32,
            burst_on_ms: 350 + ((seed >> 9) % 350) as i32,
            burst_off_ms: 200 + ((seed >> 25) % 250) as i32,
        }
    }

    pub fn report(&self, obs: &BotObservation, intent: &BotIntent, cmd: &UserCmd) -> MotorReport {
        if matches!(intent.move_mode, MoveMode::Mantle | MoveMode::Ladder) {
            return MotorReport::Unsupported;
        }
        if intent.path == PathOutcome::BudgetExhausted || intent.path == PathOutcome::ProgressLost {
            return MotorReport::Executing;
        }
        if intent.path == PathOutcome::Blocked || intent.path == PathOutcome::Unreachable {
            return MotorReport::Blocked;
        }
        if intent.move_mode == MoveMode::Drop {
            let Some(goal) = intent.move_goal else {
                return MotorReport::Executing;
            };
            if obs.self_state.origin[2] - goal[2] > crate::nav::STEP_Z_IN {
                return MotorReport::Executing;
            }
            if dist2(obs.self_state.origin, goal) <= ARRIVE_IN * ARRIVE_IN {
                return MotorReport::Arrived;
            }
            return MotorReport::Executing;
        }
        if intent.move_mode == MoveMode::Hold
            || (cmd.forwardmove == 0 && (cmd.rightmove == 0 || self.strafing))
        {
            if let Some(goal) = intent.move_goal
                && dist2(obs.self_state.origin, goal) <= ARRIVE_IN * ARRIVE_IN
            {
                return MotorReport::Arrived;
            }
            return MotorReport::Executing;
        }
        MotorReport::Executing
    }

    pub fn drive(&mut self, obs: &BotObservation, intent: &BotIntent, dt_ms: i32) -> UserCmd {
        if self.armed != Some(obs.self_state.life_sequence) {
            self.yaw = obs.self_state.viewangles[1];
            self.pitch = angle_subtract(obs.self_state.viewangles[0], 0.0);
            self.armed = Some(obs.self_state.life_sequence);
        }
        let dt = dt_ms.max(1) as f32;
        if let Some(at) = intent.look_at {
            if self
                .last_look
                .is_none_or(|old| dist2(old, at) > 48.0 * 48.0)
            {
                self.acquire_yaw = 6.0;
            }
            self.last_look = Some(at);
            self.acquire_yaw *= 0.85_f32.powf(dt / 16.0);
            let desire = look_angles(obs.eye(), at);
            let t = obs.time_ms.max(0) as f32 / 1000.0;
            // Small natural wobble of the crosshair, different for every bot.
            let wobble_yaw = 0.7
                * ((t * 1.3 + self.phase[0]).sin() + 0.5 * (t * 2.9 + self.phase[1]).sin());
            let wobble_pitch = 0.35
                * ((t * 1.7 + self.phase[2]).sin() + 0.5 * (t * 3.3 + self.phase[3]).sin());
            // While walking and not in a fight, glance to the side now and then.
            let glance = if intent.fire {
                0.0
            } else {
                let cycle = obs.time_ms.max(0) / self.glance_period_ms;
                let frac = (obs.time_ms.max(0) % self.glance_period_ms) as f32 / GLANCE_MS;
                if frac < 1.0 {
                    let side = if (cycle.wrapping_mul(2654435761u32 as i32) >> 7) & 1 == 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    side * GLANCE_DEG * (std::f32::consts::PI * frac).sin()
                } else {
                    0.0
                }
            };
            let (yaw, yaw_vel) = slew_yaw(
                self.yaw,
                desire[1] + self.err_yaw + self.acquire_yaw + wobble_yaw + glance,
                self.yaw_vel,
                dt,
            );
            self.yaw = yaw;
            self.yaw_vel = yaw_vel;
            let (pitch, pitch_vel) =
                slew_pitch(self.pitch, desire[0] + self.err_pitch + wobble_pitch, self.pitch_vel, dt);
            self.pitch = pitch;
            self.pitch_vel = pitch_vel;
        } else {
            self.yaw_vel = 0.0;
            self.pitch_vel = 0.0;
            self.last_look = None;
        }
        let unsupported = matches!(intent.move_mode, MoveMode::Mantle | MoveMode::Ladder);
        let walking = matches!(intent.move_mode, MoveMode::Walk | MoveMode::Drop)
            && matches!(intent.path, PathOutcome::Clear | PathOutcome::ProgressLost);
        let (mut forwardmove, mut rightmove) = (0i8, 0i8);
        if !unsupported
            && walking
            && let Some(goal) = intent.move_goal
        {
            let wish = [
                goal[0] - obs.self_state.origin[0],
                goal[1] - obs.self_state.origin[1],
            ];
            (forwardmove, rightmove) = wish_to_move(self.yaw, wish);
        }
        // Shooting at someone close while otherwise standing still: sidestep in short
        // alternating steps so the bot does not stand still in a fight.
        self.strafing = intent.fire
            && forwardmove == 0
            && rightmove == 0
            && !unsupported
            && !intent.crouch
            && !intent.reload
            && !intent.use_button
            && intent
                .look_at
                .is_some_and(|at| dist2(obs.eye(), at) < STRAFE_MAX_RANGE * STRAFE_MAX_RANGE);
        if self.strafing {
            rightmove = if (obs.time_ms.max(0) / self.strafe_period_ms) % 2 == 0 {
                STRAFE_SPEED
            } else {
                -STRAFE_SPEED
            };
        }
        // A requested weapon has to survive the whole change: the simulator
        // settles a dropping hand back to ready when the command asks for the
        // weapon it is already lowering.
        let weapon = intent.weapon.unwrap_or(obs.self_state.weapon);
        let delta = obs.self_state.delta_angles;
        let mut cmd = UserCmd {
            server_time: obs.time_ms,
            angles: [
                ((self.pitch - delta[0]) * ANGLE2SHORT) as i32,
                ((self.yaw - delta[1]) * ANGLE2SHORT) as i32,
                0,
            ],
            forwardmove,
            rightmove,
            weapon,
            weapon_mapped: weapon,
            ..UserCmd::default()
        };
        let on_target = self.weapon_on_target(obs, intent);
        let burst = self.burst_open(obs, intent);
        if intent.fire && on_target && burst {
            cmd.buttons |= buttons::ATTACK;
        } else if intent.fire {
            // WASM-SPIKE diagnostic (temporary)
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 6 == 0 {
                let d = intent.look_at.map(|at| look_angles(obs.eye(), at));
                diag::info!(Sim, "bots holdfire id={} t={} strafing={} on_target={on_target} burst={burst} weapon={} ammo={} yaw={:.1} pitch={:.1} desire={:?} dist={:?}", obs.self_state.id.0, obs.time_ms, self.strafing, obs.self_state.weapon, obs.self_state.ammo_clip, self.yaw, self.pitch, d, intent.look_at.map(|at| dist2(obs.eye(), at).sqrt()));
            }
        }
        if intent.use_button {
            cmd.buttons |= buttons::USE;
        }
        if intent.reload {
            cmd.buttons |= buttons::RELOAD;
        }
        if intent.sprint
            && walking
            && intent.move_mode == MoveMode::Walk
            && forwardmove > 110
            && rightmove.abs() < 45
            && !intent.fire
            && !intent.use_button
            && !intent.reload
            && !intent.crouch
        {
            cmd.buttons |= buttons::SPRINT;
        }
        if intent.crouch {
            cmd.buttons |= buttons::CROUCH;
        }
        cmd
    }

    /// Far targets get bursts with short pauses instead of a held trigger.
    fn burst_open(&self, obs: &BotObservation, intent: &BotIntent) -> bool {
        let Some(at) = intent.look_at else {
            return true;
        };
        if dist2(obs.eye(), at) < BURST_MIN_RANGE * BURST_MIN_RANGE {
            return true;
        }
        let cycle = self.burst_on_ms + self.burst_off_ms;
        obs.time_ms.max(0) % cycle < self.burst_on_ms
    }

    fn weapon_on_target(&self, obs: &BotObservation, intent: &BotIntent) -> bool {
        let Some(at) = intent.look_at else {
            return false;
        };
        let desire = look_angles(obs.eye(), at);
        angle_subtract(desire[1], self.yaw).abs() <= FIRE_CONE_DEG
            && (desire[0] - self.pitch).abs() <= FIRE_CONE_DEG
    }
}

fn look_angles(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let mut angles = math_iw4::vect_to_angles([to[0] - from[0], to[1] - from[1], to[2] - from[2]]);
    // vect_to_angles wraps upward pitch into [0, 360); the motor clamps
    // and compares pitch in the signed range.
    angles[0] = angle_subtract(angles[0], 0.0);
    angles
}

fn slew_yaw(current: f32, desire: f32, vel: f32, dt: f32) -> (f32, f32) {
    let err = angle_subtract(desire, current);
    let (delta, vel) = slew_vel(err, vel, MAX_YAW_DEG_PER_MS, MAX_YAW_ACCEL_DEG_PER_MS2, dt);
    (angle_normalize_360(current + delta), vel)
}

fn slew_pitch(current: f32, desire: f32, vel: f32, dt: f32) -> (f32, f32) {
    let desire = desire.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    let err = desire - current;
    let (delta, vel) = slew_vel(
        err,
        vel,
        MAX_PITCH_DEG_PER_MS,
        MAX_PITCH_ACCEL_DEG_PER_MS2,
        dt,
    );
    ((current + delta).clamp(-PITCH_LIMIT, PITCH_LIMIT), vel)
}

fn slew_vel(err: f32, vel: f32, max_rate: f32, max_accel: f32, dt: f32) -> (f32, f32) {
    let want = (err / dt).clamp(-max_rate, max_rate);
    let vel =
        (vel + (want - vel).clamp(-max_accel * dt, max_accel * dt)).clamp(-max_rate, max_rate);
    (vel * dt, vel)
}

fn wish_to_move(yaw_deg: f32, wish: [f32; 2]) -> (i8, i8) {
    let len = (wish[0] * wish[0] + wish[1] * wish[1]).sqrt();
    if len < 1.0 {
        return (0, 0);
    }
    let (forward, right) = yaw_vectors_2d(yaw_deg);
    let f = (wish[0] * forward[0] + wish[1] * forward[1]) / len;
    let r = (wish[0] * right[0] + wish[1] * right[1]) / len;
    (
        (f * 127.0).round().clamp(-127.0, 127.0) as i8,
        (r * 127.0).round().clamp(-127.0, 127.0) as i8,
    )
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}
