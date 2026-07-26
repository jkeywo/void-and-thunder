//! The orbit camera rig: follows the player ship, snaps to the active aim mode
//! (broadside / top-down kit), free-looks otherwise, and carries screen-shake.

use bevy::prelude::*;
use bevy::time::Real;
use std::f32::consts::PI;
use vt_sim::prelude::*;

use crate::data::feel::CameraFeel;
use crate::data::FeelTuning;
use crate::input::{deadzone, Aiming, InputMethod, Paused, PlayerAi};
use crate::session::GameState;
use crate::Player;

/// Marker for the camera so we can make it orbit the player.
#[derive(Component)]
pub struct MainCamera;

// Where the camera sits, how quickly it gets there, and how it shakes is all
// authored in `FeelTuning::camera` (see `data/feel.rs`), so the whole rig can be
// reshaped with the game running. The one value below stays compiled in: it is
// structural rather than tuning.

/// How far the gamepad free-look yaw may swing from dead-astern (radians) — a
/// full half-turn, so the view can be swung all the way to the ship's front.
const LOOK_YAW_LIMIT: f32 = PI;

/// Persistent free-look offset for gamepad camera control. The right stick nudges
/// these like a mouse (rate, not absolute); the camera yaw sits at
/// `heading + yaw_offset` and pitch at `pitch`. Mouse look stays absolute and
/// ignores this.
#[derive(Resource)]
pub struct FreeLook {
    yaw_offset: f32,
    pitch: f32,
    /// Seconds since the player last moved the look control. After
    /// `CameraFeel::recenter_delay` the view eases back to its default trailing
    /// position.
    idle: f32,
    /// Last cursor position, to detect mouse look movement.
    last_cursor: Vec2,
}

impl Default for FreeLook {
    fn default() -> Self {
        Self {
            yaw_offset: 0.0,
            pitch: CameraFeel::default().pitch_base,
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
    /// A directional shove from an impact, decaying back to zero. Unlike
    /// `trauma` — which is undirected noise — this carries *where* the blow came
    /// from, so taking a broadside to the flank visibly knocks the view aside.
    kick: Vec3,
    /// Eased vertical field of view, widened while boosting.
    fov: f32,
    /// Attract-screen orbit angle, advanced only while on the start screen.
    menu_orbit: f32,
    seed: u32,
    /// Shake noise, sampled at a fixed rate and interpolated between samples.
    ///
    /// Drawing a fresh random offset every frame is white noise: it looks like
    /// static, and worse, it changes character with framerate — the same trauma
    /// reads as a different shake at 60 and 144 Hz. Holding a target and easing
    /// toward it gives a shake with a *frequency*, which is what makes it read
    /// as an impact travelling through the hull.
    shake_from: Vec3,
    shake_to: Vec3,
    shake_phase: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // Seeded from the compiled-in feel; `camera_orbit` eases from here to
        // whatever the loaded file says on the first frame.
        let default_cam = CameraFeel::default();
        Self {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: default_cam.pitch_base,
            dist: default_cam.distance,
            trauma: 0.0,
            kick: Vec3::ZERO,
            fov: default_cam.base_fov,
            menu_orbit: 0.0,
            seed: 0,
            shake_from: Vec3::ZERO,
            shake_to: Vec3::ZERO,
            shake_phase: 0.0,
        }
    }
}

impl CameraRig {
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
    }

    /// Shove the eye `amount` world units along `dir` — the direction the blow
    /// was travelling. Kicks accumulate, so a volley of hits piles up, but never
    /// past `max`: without the cap a torpedo salvo throws the eye off the map.
    pub fn add_kick(&mut self, dir: Vec2, amount: f32, max: f32) {
        self.kick += dir.normalize_or_zero().extend(0.0) * amount;
        self.kick = self.kick.clamp_length_max(max);
    }

    /// Where the camera is currently looking, in world space. Effects use this
    /// to fall off shake with distance from the action on screen.
    pub fn focus(&self) -> Vec2 {
        self.target
    }

    /// Next pseudo-random float in `-1.0..1.0`.
    fn noise(&mut self) -> f32 {
        lcg_next(&mut self.seed) * 2.0 - 1.0
    }

    /// Advance the shake noise by `dt` and return the current offset, in the
    /// range `-1..1` per axis. Samples a new target every `1/freq` seconds and
    /// eases toward it with a smoothstep, so the motion has a defined frequency
    /// instead of being framerate-dependent hash.
    fn shake_offset(&mut self, dt: f32, freq: f32) -> Vec3 {
        self.shake_phase += dt * freq.max(0.001);
        while self.shake_phase >= 1.0 {
            self.shake_phase -= 1.0;
            self.shake_from = self.shake_to;
            // Z is halved: throwing the eye up and down as hard as sideways
            // reads as the camera bouncing rather than the ship being hit.
            self.shake_to = Vec3::new(self.noise(), self.noise(), self.noise() * 0.5);
        }
        let t = self.shake_phase;
        let smooth = t * t * (3.0 - 2.0 * t);
        self.shake_from.lerp(self.shake_to, smooth)
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
    state: Res<State<GameState>>,
    player: Query<
        (
            &Transform,
            &Heading,
            &Velocity,
            &Broadside,
            &PilotIntent,
            Option<&BoostDrive>,
        ),
        (With<Player>, Without<MainCamera>),
    >,
    feel: Res<FeelTuning>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
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
            let sx = deadzone(
                pad.get(GamepadAxis::RightStickX).unwrap_or(0.0),
                &feel.controls,
            );
            let sy = deadzone(
                pad.get(GamepadAxis::RightStickY).unwrap_or(0.0),
                &feel.controls,
            );
            look_active = sx != 0.0 || sy != 0.0;
        }
    }

    let cam = feel.camera;
    let (mut target_yaw, mut target_pitch, mut target_dist) =
        (rig.yaw, cam.pitch_base, cam.distance);
    let mut desired_focus = rig.target;
    let mut locked = false;
    let mut boosting = false;
    let on_menu = *state.get() == GameState::Menu;
    if let Ok((transform, heading, velocity, bank, pilot, boost)) = player.single() {
        let pos = transform.translation.truncate();
        boosting = boost.is_some_and(BoostDrive::engaged);
        // Look where the ship is *going*, not where it is — a short lead along
        // the velocity, capped so a boost or a warp can't fling the focus off
        // the hull entirely.
        desired_focus = pos + (velocity.0 * cam.lead_secs).clamp_length_max(cam.lead_max);
        // While the AI flies, the camera stays a free-look — it doesn't snap to
        // the AI's aiming.
        let manual = !player_ai.on;
        let aiming_broadside = aiming.port || aiming.starboard;
        if on_menu {
            // Attract screen: drift slowly around the parked ship behind the
            // title card. Focus stays on the hull itself — a lead off a frozen
            // ship would just sit the camera off-centre for no reason.
            desired_focus = pos;
            rig.menu_orbit = wrap_angle(rig.menu_orbit + cam.menu_orbit_rate * dt);
            target_yaw = heading.0 + rig.menu_orbit;
            target_pitch = cam.pitch_base;
        } else if manual && (pilot.microwarp_hold || pilot.torpedo_hold) {
            // Directly overhead and high up — a top-down pointer view. Both the
            // torpedo and microwarp modes stay centred on the ship (the aim
            // reticle moves within the view, the camera doesn't chase it).
            target_yaw = heading.0;
            target_pitch = cam.topdown_pitch;
            target_dist = cam.topdown_dist;
            locked = true;
        } else if manual && aiming_broadside {
            // Lock the yaw along where the broadside points; the aim axis steers
            // it across the arc (see `player_input`).
            let is_port = aiming.port;
            let aim_dir = (pilot.aim_point - pos).normalize_or_zero();
            let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
            target_yaw = dir.to_angle();
            target_pitch = cam.aim_pitch;
            target_dist = cam.distance * cam.aim_dist;
            locked = true;
        } else if use_pad {
            // Gamepad free-look: integrate the right stick into a persistent
            // offset (rate, like a mouse), then sit the view at heading + offset.
            if let Some(pad) = gamepads.iter().next() {
                let sx = deadzone(
                    pad.get(GamepadAxis::RightStickX).unwrap_or(0.0),
                    &feel.controls,
                );
                let sy = deadzone(
                    pad.get(GamepadAxis::RightStickY).unwrap_or(0.0),
                    &feel.controls,
                );
                freelook.yaw_offset = (freelook.yaw_offset - sx * cam.look_yaw_rate * dt)
                    .clamp(-LOOK_YAW_LIMIT, LOOK_YAW_LIMIT);
                freelook.pitch = (freelook.pitch - sy * cam.look_pitch_rate * dt)
                    .clamp(cam.pitch_min, cam.pitch_max);
            }
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        } else {
            // Mouse free-look: absolute offset from the heading (yaw not inverted).
            target_yaw = heading.0 - look_x * 0.9;
            target_pitch = (cam.pitch_base + look_y * 0.5).clamp(cam.pitch_min, cam.pitch_max);
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
        let recenter_delay = if use_pad {
            cam.recenter_delay_pad
        } else {
            cam.recenter_delay
        };
        if !locked && !on_menu && freelook.idle > recenter_delay {
            let forward = heading.forward();
            let reversing = velocity.0.dot(forward) < -5.0;
            let default_off = if reversing { PI } else { 0.0 };
            let rk = 1.0 - (-cam.recenter_lerp * dt).exp();
            freelook.yaw_offset += wrap_angle(default_off - freelook.yaw_offset) * rk;
            freelook.pitch += (cam.pitch_base - freelook.pitch) * rk;
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        }
    }

    let lerp = if locked { cam.aim_lerp } else { cam.yaw_lerp };
    let k = 1.0 - (-lerp * dt).exp();
    rig.yaw = wrap_angle(rig.yaw + wrap_angle(target_yaw - rig.yaw) * k);
    rig.pitch += (target_pitch - rig.pitch) * k;
    let dk = 1.0 - (-cam.dist_lerp * dt).exp();
    rig.dist += (target_dist - rig.dist) * dk;
    // Ease the focus toward the ship (or the warp point) tightly.
    let fk = 1.0 - (-cam.focus_lerp * dt).exp();
    let focus_step = (desired_focus - rig.target) * fk;
    rig.target += focus_step;

    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - dt * cam.trauma_decay).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let shake = rig.shake_offset(dt, cam.shake_freq) * cam.shake_magnitude * amount;
    // Impact kick: a directional shove that springs back, on top of the noise.
    rig.kick *= (-cam.kick_decay * dt).exp();
    let kick = rig.kick;

    // Widen the view as the boost drive bites, then relax.
    let target_fov = if boosting {
        cam.base_fov + cam.boost_fov_gain
    } else {
        cam.base_fov
    };
    rig.fov += (target_fov - rig.fov) * (1.0 - (-cam.fov_lerp * dt).exp());

    let Ok((mut camera, mut projection)) = camera.single_mut() else {
        return;
    };
    if let Projection::Perspective(perspective) = &mut *projection {
        perspective.fov = rig.fov;
    }
    // As pitch rises the camera lifts (sin) and swings overhead (cos shrinks the
    // horizontal reach); the eased distance sets how far out the eye sits.
    let look = Vec2::from_angle(rig.yaw);
    let back = Vec3::new(-look.x, -look.y, 0.0);
    let focus = rig.target.extend(0.0);
    let eye =
        focus + (back * rig.pitch.cos() + Vec3::Z * rig.pitch.sin()) * rig.dist + shake + kick;
    *camera = Transform::from_translation(eye).looking_at(focus, Vec3::Z);
}
