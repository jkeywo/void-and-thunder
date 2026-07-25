//! Input: translating keyboard/mouse/gamepad into the player ship's sim intent,
//! plus the small resources (aim state, active input method, pause/AI toggles)
//! that both `player_input` and the camera/HUD read.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use std::f32::consts::FRAC_PI_2;
use vt_sim::prelude::*;

use crate::camera::MainCamera;
use crate::Player;

/// Which broadsides the player is currently *holding* (aiming). Purely
/// presentational — it drives the aim beam; firing happens on release.
#[derive(Resource, Default)]
pub struct Aiming {
    pub port: bool,
    pub starboard: bool,
}

/// The most recently used input device, so the HUD can show matching hints.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMethod {
    #[default]
    KeyboardMouse,
    Gamepad,
}

/// Whether the player ship is currently flown by the AI (toggle with T). The
/// camera stays under the player's control either way.
#[derive(Resource, Default)]
pub struct PlayerAi {
    pub on: bool,
}

/// Whether the game is paused. Pausing freezes `Time<Virtual>` (and with it the
/// whole `FixedUpdate` simulation and every virtual-time visual); real-time
/// input still flows so the pause can be lifted.
#[derive(Resource, Default)]
pub struct Paused(pub bool);

/// Whether the HUD's controls side panel is open (toggled with Tab).
#[derive(Resource, Default)]
pub struct ControlsPanel {
    pub open: bool,
}

/// The broadside arc offset (-1..1 across the bank's arc) while a broadside is
/// held, driven by yaw *input*: the gamepad stick sets it absolutely
/// (spring-centred); the mouse accumulates motion into it so you steer with
/// movement and each aim begins centred on the beam. Reset to 0 whenever no
/// broadside is held, so the aim never sticks wherever the cursor last sat.
#[derive(Resource, Default)]
pub struct BroadsideAim {
    offset: f32,
}

/// The world point the aim reticle sits on for the pointer-aimed kit (EMP,
/// torpedo, microwarp). The mouse sets it absolutely (plane pick); the gamepad
/// nudges it at a rate (like a mouse). It rests just ahead of the bow whenever
/// nothing is being aimed, so each aim starts from a sensible spot.
#[derive(Resource, Default)]
pub struct AimCursor {
    pub world: Vec2,
}

/// How much a pixel of mouse motion sweeps the broadside arc offset (-1..1). At
/// ~0.0032 a rightward drag of ~310px covers the full half-arc. Motion-driven,
/// not cursor-position-driven, so the aim never sticks where the cursor sat.
const MOUSE_AIM_SENS: f32 = 0.0032;
/// Gamepad aim-pointer speed (world units/sec at full stick) for the top-down
/// torpedo / microwarp pointer and the EMP aim — rate-based, like a mouse.
const AIM_CURSOR_RATE: f32 = 780.0;
/// How far from the ship the gamepad aim pointer may stray.
const AIM_CURSOR_MAX: f32 = 1300.0;

/// Radial deadzone below which the stick reads zero, above which it is rescaled
/// so motion starts smoothly just past the edge (5% → ~0) and *saturates* before
/// the physical limit (≥ 90% → 100%).
///
/// The saturation matters: a real stick rarely reports a clean 1.0 on one axis —
/// the gate is round and the springs wear — so without it a pad could never
/// reach the full turn rate a keyboard gets for free, and steering felt sluggish
/// by comparison.
pub fn deadzone(v: f32) -> f32 {
    const DZ: f32 = 0.05;
    const SATURATION: f32 = 0.90;
    let a = v.abs();
    if a < DZ {
        0.0
    } else {
        v.signum() * ((a - DZ) / (SATURATION - DZ)).min(1.0)
    }
}

/// One frame's aim decision: where the reticle sits, and the broadside arc
/// offset to steer by / fire along.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AimDecision {
    pub aim_point: Vec2,
    pub broadside_offset: f32,
}

/// Decide the ship's aim point and broadside arc offset for one frame, mirroring
/// `ai::desired_helm`'s pure-function discipline: `player_input` gathers raw
/// device state (mouse motion, stick axes, a plane-pick raycast) and hands it
/// here as plain values — nothing below touches `Res`/`Query`.
///
/// `yaw_input` is the broadside arc-offset control: an absolute stick value on
/// gamepad (spring-centred), a motion delta to accumulate on mouse.
/// `pad_cursor_delta` is the gamepad's world-space rate nudge for the pointer
/// kit this frame (zero when not using a pad). `mouse_pick` is the resolved
/// mouse plane-pick (already defaulted to `rest_point` by the caller on a
/// raycast miss). `rest_point` is where the reticle snaps when idle.
#[allow(clippy::too_many_arguments)]
pub fn decide_aim(
    ship: Vec2,
    heading: f32,
    bank_arc: f32,
    aim_port: bool,
    aim_starboard: bool,
    fire_port: bool,
    fire_starboard: bool,
    kit_active: bool,
    use_pad: bool,
    prior_broadside_offset: f32,
    prior_aim_cursor: Vec2,
    yaw_input: f32,
    pad_cursor_delta: Vec2,
    mouse_pick: Vec2,
    rest_point: Vec2,
    cursor_max: f32,
) -> AimDecision {
    let aiming_broadside = aim_port || aim_starboard;

    // Recompute while aiming; preserve through a release frame (so the fire
    // direction still matches where you were pointing); reset once genuinely
    // idle. See `broadside_fire_direction` for why "preserve" matters.
    let broadside_offset = if aiming_broadside {
        if use_pad {
            yaw_input.clamp(-1.0, 1.0)
        } else {
            (prior_broadside_offset + yaw_input).clamp(-1.0, 1.0)
        }
    } else if fire_port || fire_starboard {
        prior_broadside_offset
    } else {
        0.0
    };

    let aim_point = if aiming_broadside {
        let beam = heading + if aim_port { FRAC_PI_2 } else { -FRAC_PI_2 };
        let angle = beam - broadside_offset * bank_arc;
        ship + Vec2::from_angle(angle) * 320.0
    } else if use_pad {
        if kit_active {
            let mut cursor = prior_aim_cursor + pad_cursor_delta;
            let off = cursor - ship;
            if off.length() > cursor_max {
                cursor = ship + off.normalize_or_zero() * cursor_max;
            }
            cursor
        } else {
            rest_point
        }
    } else {
        mouse_pick
    };

    AimDecision {
        aim_point,
        broadside_offset,
    }
}

/// The world direction a fired broadside should travel, using the *held* arc
/// offset rather than the live aim point — on the release frame the button is
/// already up, so the live aim point has snapped back toward idle; firing off
/// that would miss. `is_port` selects which beam.
pub fn broadside_fire_direction(heading: f32, is_port: bool, held_offset: f32, arc: f32) -> Vec2 {
    let beam = heading + if is_port { FRAC_PI_2 } else { -FRAC_PI_2 };
    Vec2::from_angle(beam - held_offset * arc)
}

/// Translate keyboard **and** gamepad into the player ship's helm, fire
/// requests, brace and boarding intent.
///
/// Broadsides are **hold to aim, release to fire**: while a side's button is
/// held we only show the aim beam; the release edge raises the sim's
/// [`FireOrders`] request, which `weapons_system` consumes exactly once.
#[allow(clippy::too_many_arguments)]
pub fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    real: Res<Time<Real>>,
    method: Res<InputMethod>,
    paused: Res<Paused>,
    player_ai: Res<PlayerAi>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mut board: ResMut<BoardIntent>,
    mut aiming: ResMut<Aiming>,
    mut aim_cursor: ResMut<AimCursor>,
    mut broadside_aim: ResMut<BroadsideAim>,
    mut player: Query<
        (
            &mut Helm,
            &mut FireOrders,
            &mut Brace,
            &mut BoostDrive,
            &mut PilotIntent,
            &Transform,
            &Heading,
            &Broadside,
            &MicrowarpDrive,
        ),
        With<Player>,
    >,
) {
    // Paused, or the AI is flying — the client stays hands-off (the camera is
    // handled separately). When the AI flies, the sim writes the ship's intent.
    if paused.0 || player_ai.on {
        return;
    }
    let Ok((mut helm, mut orders, mut brace, mut boost, mut pilot, transform, heading, bank, warp)) =
        player.single_mut()
    else {
        return;
    };

    // --- Keyboard ---
    let mut throttle = 0.0;
    if keys.pressed(KeyCode::KeyW) {
        throttle += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        throttle -= 1.0;
    }

    let mut turn = 0.0;
    if keys.pressed(KeyCode::KeyA) {
        turn += 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        turn -= 1.0;
    }

    // Broadsides on the mouse buttons; EMP on Q.
    let mut aim_port = mouse.pressed(MouseButton::Left);
    let mut aim_starboard = mouse.pressed(MouseButton::Right);
    let mut fire_port = mouse.just_released(MouseButton::Left);
    let mut fire_starboard = mouse.just_released(MouseButton::Right);
    let mut emp_fire = keys.pressed(KeyCode::KeyQ);
    let mut torpedo_hold = keys.pressed(KeyCode::ControlLeft);
    let mut microwarp_hold = keys.pressed(KeyCode::ShiftLeft);
    let mut bracing = keys.pressed(KeyCode::KeyC);
    let mut boosting = keys.pressed(KeyCode::Space);
    let mut board_now = keys.just_pressed(KeyCode::KeyB);

    // --- Gamepad (first connected pad): the final scheme ---
    let pad = gamepads.iter().next();
    if let Some(pad) = pad {
        throttle += deadzone(pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0));
        // Stick right (+X) steers starboard (negative turn).
        turn -= deadzone(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0));

        aim_port |= pad.pressed(GamepadButton::LeftTrigger2); // LT
        aim_starboard |= pad.pressed(GamepadButton::RightTrigger2); // RT
        fire_port |= pad.just_released(GamepadButton::LeftTrigger2);
        fire_starboard |= pad.just_released(GamepadButton::RightTrigger2);
        torpedo_hold |= pad.pressed(GamepadButton::LeftTrigger); // LB
        microwarp_hold |= pad.pressed(GamepadButton::RightTrigger); // RB
        emp_fire |= pad.pressed(GamepadButton::West); // X / Square
        bracing |= pad.pressed(GamepadButton::North); // Y / Triangle
        boosting |= pad.pressed(GamepadButton::South); // A / Cross
        board_now |= pad.just_pressed(GamepadButton::East); // B / Circle
    }

    helm.throttle = throttle.clamp(-1.0, 1.0);
    helm.turn = turn.clamp(-1.0, 1.0);
    aiming.port = aim_port;
    aiming.starboard = aim_starboard;

    // The microwarp can't even be *aimed* while it's recharging — suppress the
    // hold so the top-down view, ghost preview and aim-battery drain never engage
    // on cooldown. (The sim also gates the warp itself on the same timer.)
    let microwarp_hold = microwarp_hold && warp.timer <= 0.0;

    // --- Aim: gather raw device state, then hand off to the pure decision ---
    let ship = transform.translation.truncate();
    let dt = real.delta_secs();
    let use_pad = *method == InputMethod::Gamepad;
    let aiming_broadside = aim_port || aim_starboard;
    let kit_active = emp_fire || torpedo_hold || microwarp_hold;
    let rest_point = ship + heading.forward() * 320.0;
    let right_stick = |axis: GamepadAxis| pad.map_or(0.0, |p| deadzone(p.get(axis).unwrap_or(0.0)));

    let yaw_input = if use_pad {
        right_stick(GamepadAxis::RightStickX)
    } else {
        mouse_motion.delta.x * MOUSE_AIM_SENS
    };

    let pad_cursor_delta = if use_pad && kit_active {
        if let Ok((_, cam_gt)) = camera_q.single() {
            let right = cam_gt.right().truncate().normalize_or_zero();
            let up = cam_gt.up().truncate().normalize_or_zero();
            let sx = right_stick(GamepadAxis::RightStickX);
            let sy = right_stick(GamepadAxis::RightStickY);
            (right * sx + up * sy) * AIM_CURSOR_RATE * dt
        } else {
            Vec2::ZERO
        }
    } else {
        Vec2::ZERO
    };

    // The mouse's plane pick, defaulting to `rest_point` when the ray misses /
    // off-screen. Only needed off-pad — skip the raycast on gamepad.
    let mouse_pick = if use_pad {
        Vec2::ZERO
    } else if let (Ok((camera, cam_gt)), Ok(window)) = (camera_q.single(), windows.single()) {
        window
            .cursor_position()
            .and_then(|cursor| camera.viewport_to_world(cam_gt, cursor).ok())
            .and_then(|ray| {
                ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Z))
                    .map(|d| ray.get_point(d).truncate())
            })
            .unwrap_or(rest_point)
    } else {
        rest_point
    };

    let decision = decide_aim(
        ship,
        heading.0,
        bank.arc,
        aim_port,
        aim_starboard,
        fire_port,
        fire_starboard,
        kit_active,
        use_pad,
        broadside_aim.offset,
        aim_cursor.world,
        yaw_input,
        pad_cursor_delta,
        mouse_pick,
        rest_point,
        AIM_CURSOR_MAX,
    );
    broadside_aim.offset = decision.broadside_offset;
    if !aiming_broadside {
        // While aiming a broadside the pointer cursor is irrelevant — leave it
        // as last set, so switching back to a kit weapon resumes from there.
        aim_cursor.world = decision.aim_point;
    }

    if fire_port {
        orders.port = true;
        orders.aim = Some(broadside_fire_direction(
            heading.0,
            true,
            decision.broadside_offset,
            bank.arc,
        ));
    }
    if fire_starboard {
        orders.starboard = true;
        orders.aim = Some(broadside_fire_direction(
            heading.0,
            false,
            decision.broadside_offset,
            bank.arc,
        ));
    }
    brace.active = bracing;
    boost.active = boosting;
    pilot.aim_point = decision.aim_point;
    pilot.emp_fire = emp_fire;
    pilot.torpedo_hold = torpedo_hold;
    pilot.microwarp_hold = microwarp_hold;
    if board_now {
        board.active = true;
    }
}

/// Toggle AI control of the player ship with `T`. Enabling it drops an
/// `AiController` (piloting preset) onto the player so the sim's AI flies it;
/// disabling it removes the controller and hands the ship back. The camera is
/// never affected — the player always steers the view.
pub fn toggle_player_ai(
    keys: Res<ButtonInput<KeyCode>>,
    mut player_ai: ResMut<PlayerAi>,
    mut aiming: ResMut<Aiming>,
    mut commands: Commands,
    player: Query<Entity, With<Player>>,
) {
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    player_ai.on = !player_ai.on;
    let Ok(entity) = player.single() else {
        return;
    };
    if player_ai.on {
        commands.entity(entity).insert(AiController::piloting());
        *aiming = Aiming::default(); // clear any held broadside aim
    } else {
        commands.entity(entity).remove::<AiController>();
    }
}

/// Toggle pause with `P` / `Escape` (or the pad's Start). Pausing freezes
/// `Time<Virtual>`, which halts the whole `FixedUpdate` simulation and every
/// virtual-time visual at once; real-time input keeps flowing so the pause can
/// be lifted.
pub fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut paused: ResMut<Paused>,
    mut virt: ResMut<Time<Virtual>>,
) {
    let pad_toggle = gamepads
        .iter()
        .any(|pad| pad.just_pressed(GamepadButton::Start));
    if !keys.just_pressed(KeyCode::KeyP) && !keys.just_pressed(KeyCode::Escape) && !pad_toggle {
        return;
    }
    paused.0 = !paused.0;
    if paused.0 {
        virt.pause();
    } else {
        virt.unpause();
    }
}

/// Toggle the HUD's controls side panel with `Tab`. Purely presentational — it
/// doesn't pause or otherwise affect the sim.
pub fn toggle_controls_panel(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<ControlsPanel>) {
    if keys.just_pressed(KeyCode::Tab) {
        panel.open = !panel.open;
    }
}

/// Track the last-used input device so the HUD shows matching control hints.
pub fn track_input_method(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut method: ResMut<InputMethod>,
) {
    let axes = [
        GamepadAxis::LeftStickX,
        GamepadAxis::LeftStickY,
        GamepadAxis::RightStickX,
        GamepadAxis::RightStickY,
    ];
    let buttons = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::LeftTrigger2,
        GamepadButton::RightTrigger2,
        GamepadButton::Start,
    ];
    let pad_active = gamepads.iter().any(|pad| {
        axes.iter().any(|a| pad.get(*a).unwrap_or(0.0).abs() > 0.3)
            || buttons.iter().any(|b| pad.pressed(*b))
    });
    if pad_active {
        *method = InputMethod::Gamepad;
    } else if keys.get_pressed().next().is_some() || mouse.get_pressed().next().is_some() {
        *method = InputMethod::KeyboardMouse;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARC: f32 = 0.6;

    #[test]
    fn decide_aim_defaults_to_broadside_mode_while_holding_a_side() {
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            true,  // aim_port
            false, // aim_starboard
            false,
            false,
            false, // kit_active
            false, // use_pad
            0.0,
            Vec2::ZERO,
            0.2, // yaw_input: mouse delta
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            1300.0,
        );
        assert!((d.broadside_offset - 0.2).abs() < 1e-4);
        // Port beam at heading 0 is +Y; a positive offset sweeps it toward +X.
        assert!(d.aim_point.y > 0.0);
    }

    #[test]
    fn decide_aim_preserves_the_offset_through_a_release_frame() {
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false, // aim_port: already released this frame
            false,
            true, // fire_port
            false,
            false,
            false,
            0.35, // prior offset from the frame that was still aiming
            Vec2::ZERO,
            0.0,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            1300.0,
        );
        assert!((d.broadside_offset - 0.35).abs() < 1e-6);
    }

    #[test]
    fn decide_aim_resets_the_offset_once_genuinely_idle() {
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false,
            false,
            false,
            false,
            false,
            false,
            0.35,
            Vec2::ZERO,
            0.0,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            1300.0,
        );
        assert_eq!(d.broadside_offset, 0.0);
    }

    #[test]
    fn decide_aim_falls_back_to_pointer_mode_for_kit_weapons() {
        let pick = Vec2::new(120.0, 40.0);
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false,
            false,
            false,
            false,
            true,  // kit_active
            false, // use_pad: mouse drives the pick directly
            0.0,
            Vec2::ZERO,
            0.0,
            Vec2::ZERO,
            pick,
            Vec2::new(320.0, 0.0),
            1300.0,
        );
        assert_eq!(d.aim_point, pick);
    }

    #[test]
    fn decide_aim_rests_ahead_of_the_bow_when_idle_on_gamepad() {
        let rest = Vec2::new(320.0, 0.0);
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false,
            false,
            false,
            false,
            false, // kit_active: idle, not holding a kit weapon
            true,  // use_pad
            0.0,
            Vec2::new(999.0, 999.0), // a stale cursor from an earlier aim
            0.0,
            Vec2::ZERO,
            Vec2::ZERO,
            rest,
            1300.0,
        );
        // Idle gamepad snaps straight to rest — it doesn't integrate the stale cursor.
        assert_eq!(d.aim_point, rest);
    }

    #[test]
    fn decide_aim_integrates_the_gamepad_cursor_delta_while_using_the_kit() {
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false,
            false,
            false,
            false,
            true, // kit_active
            true, // use_pad
            0.0,
            Vec2::new(100.0, 0.0), // prior cursor
            0.0,
            Vec2::new(10.0, 0.0), // this frame's rate delta
            Vec2::ZERO,
            Vec2::ZERO,
            1300.0,
        );
        assert_eq!(d.aim_point, Vec2::new(110.0, 0.0));
    }

    #[test]
    fn decide_aim_clamps_the_gamepad_cursor_to_its_max_range() {
        let d = decide_aim(
            Vec2::ZERO,
            0.0,
            ARC,
            false,
            false,
            false,
            false,
            true,
            true,
            0.0,
            Vec2::new(1290.0, 0.0),
            0.0,
            Vec2::new(50.0, 0.0),
            Vec2::ZERO,
            Vec2::ZERO,
            1300.0,
        );
        assert!((d.aim_point.length() - 1300.0).abs() < 1e-3);
    }

    #[test]
    fn broadside_fire_direction_matches_the_held_offset_not_live_aim() {
        // Facing +X (heading 0), port beam is +Y; a positive offset sweeps
        // toward +X same as decide_aim's broadside branch.
        let dir = broadside_fire_direction(0.0, true, 0.0, ARC);
        assert!(dir.y > 0.9);
    }
}
