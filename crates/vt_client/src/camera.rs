//! The orbit camera rig: follows the player ship, snaps to the active aim mode
//! (broadside / top-down kit), free-looks otherwise, and carries screen-shake.

use bevy::prelude::*;
use bevy::time::Real;
use std::f32::consts::PI;
use vt_sim::prelude::*;

use crate::input::{deadzone, Aiming, InputMethod, Paused, PlayerAi};
use crate::Player;

/// Marker for the camera so we can make it orbit the player.
#[derive(Component)]
pub struct MainCamera;

/// How far behind the ship the camera sits.
pub const CAM_DISTANCE: f32 = 430.0;
/// How high above the plane the camera sits — low, so the view is a shallow
/// "slightly above, pointing slightly down" angle rather than top-down.
pub const CAM_HEIGHT: f32 = 170.0;
/// How quickly the camera yaw catches up to the ship's heading.
const CAM_YAW_LERP: f32 = 2.5;
/// Camera pitch (radians above horizontal): resting, minimum, maximum, and the
/// value it eases to while aiming a broadside. As pitch rises the camera is
/// raised and pulled in toward the ship (see `camera_orbit`).
const CAM_PITCH_BASE: f32 = 0.85;
const CAM_PITCH_MIN: f32 = 0.35;
const CAM_PITCH_MAX: f32 = 1.2;
const CAM_AIM_PITCH: f32 = 0.72;
/// How much the camera pulls in toward the ship at maximum pitch (0 = none).
const CAM_PITCH_ZOOM: f32 = 0.4;
/// Near-top-down pitch used while aiming a torpedo volley or a microwarp — the
/// camera rises directly over the ship and the aim becomes a top-down pointer.
const CAM_TOPDOWN_PITCH: f32 = 1.5;
/// How high the camera pulls out for the top-down (torpedo / microwarp) view —
/// centred on the ship, tightened to 75% of the old reach.
const CAM_TOPDOWN_DIST: f32 = 1125.0;
/// Faster yaw/pitch ease while locked to an aim (broadside / microwarp).
const CAM_AIM_LERP: f32 = 9.0;
/// How quickly the eased camera distance catches its target.
const CAM_DIST_LERP: f32 = 6.0;
/// Gamepad free-look rate (radians/sec of yaw, units/sec of pitch, at full
/// stick). The right stick moves the *view* like a mouse — deflection is a rate,
/// not an absolute offset — while not aiming a broadside.
const LOOK_YAW_RATE: f32 = 2.4;
const LOOK_PITCH_RATE: f32 = 1.8;
/// How far the gamepad free-look yaw may swing from dead-astern (radians) — a
/// full half-turn, so the view can be swung all the way to the ship's front.
const LOOK_YAW_LIMIT: f32 = PI;
/// Seconds of no look input before the camera eases back to its default trailing
/// position (behind the ship when moving forward, ahead of it when reversing).
const RECENTER_DELAY: f32 = 3.0;
/// How gently the idle camera eases back to its default position.
const RECENTER_LERP: f32 = 1.6;

/// Persistent free-look offset for gamepad camera control. The right stick nudges
/// these like a mouse (rate, not absolute); the camera yaw sits at
/// `heading + yaw_offset` and pitch at `pitch`. Mouse look stays absolute and
/// ignores this.
#[derive(Resource)]
pub struct FreeLook {
    yaw_offset: f32,
    pitch: f32,
    /// Seconds since the player last moved the look control. After
    /// [`RECENTER_DELAY`] the view eases back to its default trailing position.
    idle: f32,
    /// Last cursor position, to detect mouse look movement.
    last_cursor: Vec2,
}

impl Default for FreeLook {
    fn default() -> Self {
        Self {
            yaw_offset: 0.0,
            pitch: CAM_PITCH_BASE,
            idle: 0.0,
            last_cursor: Vec2::ZERO,
        }
    }
}

/// Camera orbit + screen-shake state. `yaw`/`pitch` are eased toward targets set
/// by free-look or the active aim mode; `trauma` decays each frame and is added
/// to by hits and explosions.
#[derive(Resource)]
pub struct CameraRig {
    target: Vec2,
    yaw: f32,
    pitch: f32,
    /// Eased eye distance from the focus, so the top-down modes can pull the
    /// camera smoothly up and out.
    dist: f32,
    trauma: f32,
    seed: u32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: CAM_PITCH_BASE,
            dist: CAM_DISTANCE,
            trauma: 0.0,
            seed: 0,
        }
    }
}

impl CameraRig {
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
    }

    /// Next pseudo-random float in `-1.0..1.0`.
    fn noise(&mut self) -> f32 {
        lcg_next(&mut self.seed) * 2.0 - 1.0
    }
}

/// Orbit the camera around the player. When aiming a broadside the view snaps to
/// the aim direction; otherwise the player free-looks with the mouse (offset
/// from screen centre) or right stick — yaw around the ship, pitch between
/// looking down at it and near-horizontal. Screen shake is added to the eye.
#[allow(clippy::too_many_arguments)]
pub fn camera_orbit(
    real: Res<Time<Real>>,
    paused: Res<Paused>,
    method: Res<InputMethod>,
    mut rig: ResMut<CameraRig>,
    mut freelook: ResMut<FreeLook>,
    aiming: Res<Aiming>,
    player_ai: Res<PlayerAi>,
    windows: Query<&Window>,
    gamepads: Query<&Gamepad>,
    player: Query<
        (&Transform, &Heading, &Velocity, &Broadside, &PilotIntent),
        (With<Player>, Without<MainCamera>),
    >,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    // Camera runs on real time so it stays responsive during bullet-time; a pause
    // freezes it by zeroing the step.
    let dt = if paused.0 { 0.0 } else { real.delta_secs() };
    let use_pad = *method == InputMethod::Gamepad;

    // Mouse free-look is absolute (cursor offset from centre); gamepad free-look
    // is rate-based (the stick nudges a persistent offset, like a mouse) and is
    // integrated only in the free-look branch below, so it never drifts while the
    // same stick is steering an aim.
    let (mut look_x, mut look_y) = (0.0f32, 0.0f32);
    let mut look_active = false;
    if let Ok(window) = windows.single() {
        if let (Some(cursor), (w, h)) =
            (window.cursor_position(), (window.width(), window.height()))
        {
            look_x = ((cursor.x / w) * 2.0 - 1.0).clamp(-1.0, 1.0);
            look_y = ((cursor.y / h) * 2.0 - 1.0).clamp(-1.0, 1.0);
            // The mouse counts as look input only while it's actually moving.
            if !use_pad && cursor.distance(freelook.last_cursor) > 1.0 {
                look_active = true;
            }
            freelook.last_cursor = cursor;
        }
    }
    if use_pad {
        if let Some(pad) = gamepads.iter().next() {
            let sx = deadzone(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0));
            let sy = deadzone(pad.get(GamepadAxis::RightStickY).unwrap_or(0.0));
            look_active = sx != 0.0 || sy != 0.0;
        }
    }

    let (mut target_yaw, mut target_pitch, mut target_dist) =
        (rig.yaw, CAM_PITCH_BASE, CAM_DISTANCE);
    let mut desired_focus = rig.target;
    let mut locked = false;
    if let Ok((transform, heading, velocity, bank, pilot)) = player.single() {
        let pos = transform.translation.truncate();
        desired_focus = pos;
        // While the AI flies, the camera stays a free-look — it doesn't snap to
        // the AI's aiming.
        let manual = !player_ai.on;
        let aiming_broadside = aiming.port || aiming.starboard;
        if manual && (pilot.microwarp_hold || pilot.torpedo_hold) {
            // Directly overhead and high up — a top-down pointer view. Both the
            // torpedo and microwarp modes stay centred on the ship (the aim
            // reticle moves within the view, the camera doesn't chase it).
            target_yaw = heading.0;
            target_pitch = CAM_TOPDOWN_PITCH;
            target_dist = CAM_TOPDOWN_DIST;
            locked = true;
        } else if manual && aiming_broadside {
            // Lock the yaw along where the broadside points; the aim axis steers
            // it across the arc (see `player_input`).
            let is_port = aiming.port;
            let aim_dir = (pilot.aim_point - pos).normalize_or_zero();
            let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
            target_yaw = dir.to_angle();
            target_pitch = CAM_AIM_PITCH;
            target_dist = CAM_DISTANCE * (1.0 - CAM_PITCH_ZOOM);
            locked = true;
        } else if use_pad {
            // Gamepad free-look: integrate the right stick into a persistent
            // offset (rate, like a mouse), then sit the view at heading + offset.
            if let Some(pad) = gamepads.iter().next() {
                let sx = deadzone(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0));
                let sy = deadzone(pad.get(GamepadAxis::RightStickY).unwrap_or(0.0));
                freelook.yaw_offset = (freelook.yaw_offset - sx * LOOK_YAW_RATE * dt)
                    .clamp(-LOOK_YAW_LIMIT, LOOK_YAW_LIMIT);
                freelook.pitch = (freelook.pitch - sy * LOOK_PITCH_RATE * dt)
                    .clamp(CAM_PITCH_MIN, CAM_PITCH_MAX);
            }
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        } else {
            // Mouse free-look: absolute offset from the heading (yaw not inverted).
            target_yaw = heading.0 - look_x * 0.9;
            target_pitch = (CAM_PITCH_BASE + look_y * 0.5).clamp(CAM_PITCH_MIN, CAM_PITCH_MAX);
        }

        // Idle auto-recenter: after a few seconds without look input, ease the
        // view back to its default trailing position — behind the ship when
        // making way ahead, ahead of it (looking back) when reversing. Aiming
        // counts as activity, so the timer restarts once an aim ends.
        if locked || look_active {
            freelook.idle = 0.0;
        } else {
            freelook.idle += dt;
        }
        if !locked && freelook.idle > RECENTER_DELAY {
            let forward = heading.forward();
            let reversing = velocity.0.dot(forward) < -5.0;
            let default_off = if reversing { PI } else { 0.0 };
            let rk = 1.0 - (-RECENTER_LERP * dt).exp();
            freelook.yaw_offset += wrap_angle(default_off - freelook.yaw_offset) * rk;
            freelook.pitch += (CAM_PITCH_BASE - freelook.pitch) * rk;
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        }
    }

    let lerp = if locked { CAM_AIM_LERP } else { CAM_YAW_LERP };
    let k = 1.0 - (-lerp * dt).exp();
    rig.yaw = wrap_angle(rig.yaw + wrap_angle(target_yaw - rig.yaw) * k);
    rig.pitch += (target_pitch - rig.pitch) * k;
    let dk = 1.0 - (-CAM_DIST_LERP * dt).exp();
    rig.dist += (target_dist - rig.dist) * dk;
    // Ease the focus toward the ship (or the warp point) tightly.
    let fk = 1.0 - (-10.0 * dt).exp();
    let focus_step = (desired_focus - rig.target) * fk;
    rig.target += focus_step;

    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - dt * 1.4).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let shake = Vec3::new(rig.noise(), rig.noise(), rig.noise() * 0.5) * 26.0 * amount;

    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    // As pitch rises the camera lifts (sin) and swings overhead (cos shrinks the
    // horizontal reach); the eased distance sets how far out the eye sits.
    let look = Vec2::from_angle(rig.yaw);
    let back = Vec3::new(-look.x, -look.y, 0.0);
    let focus = rig.target.extend(0.0);
    let eye = focus + (back * rig.pitch.cos() + Vec3::Z * rig.pitch.sin()) * rig.dist + shake;
    *camera = Transform::from_translation(eye).looking_at(focus, Vec3::Z);
}
