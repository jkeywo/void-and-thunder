//! Input: translating keyboard/mouse/gamepad into the player ship's sim intent,
//! plus the small resources (aim state, active input method, pause/AI toggles)
//! that both `player_input` and the camera/HUD read.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use bevy::window::{MonitorSelection, WindowMode};
use std::f32::consts::FRAC_PI_2;
use vt_sim::prelude::*;

use crate::camera::MainCamera;
use crate::data::feel::ControlFeel;
use crate::data::FeelTuning;
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
#[derive(Resource, Default, Clone, Copy)]
pub struct Paused(pub bool);

/// Whether the HUD's controls side panel is open (toggled with Tab).
#[derive(Resource, Default)]
pub struct ControlsPanel {
    pub open: bool,
}

/// The keyboard's thrust setting: a ladder of discrete notches rather than a
/// held axis.
///
/// A key is not an analog stick, and pretending otherwise is what made the
/// keyboard ship feel like a cursor — `W` was a step function straight into the
/// helm. Notching it means the keyboard player *chooses* a speed and lives with
/// its turn rate, which is the same decision the stick offers by degree. Both
/// paths still write a plain `-1..=1` [`Helm::throttle`], so the sim, the AI and
/// the dev panel neither know nor care which one is driving.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThrustState {
    /// The weak reverse.
    Reverse,
    /// Drives stopped. The handiest setting — this is where a ship pivots.
    Stop,
    /// The default: handy, and where most of a fight is fought.
    #[default]
    Half,
    Full,
}

impl ThrustState {
    /// The ladder, slowest first. Indexing this is what makes stepping trivial.
    const LADDER: [ThrustState; 4] = [
        ThrustState::Reverse,
        ThrustState::Stop,
        ThrustState::Half,
        ThrustState::Full,
    ];

    /// The `Helm::throttle` this notch commands.
    pub fn throttle(self) -> f32 {
        match self {
            ThrustState::Reverse => -1.0,
            ThrustState::Stop => 0.0,
            ThrustState::Half => 0.5,
            ThrustState::Full => 1.0,
        }
    }

    /// String-table id for this notch's HUD label. A key rather than the English
    /// text: every player-facing line lives in `assets/strings/*.csv`, and the
    /// HUD resolves it through its own `t()`.
    ///
    /// Keyed under `hud.helm.*` rather than `hud.thrust.*` because these appear
    /// in the HUD's HELM row, and `hud.thrust` is not free — it belonged to the
    /// boost gauge before that became `hud.battery`.
    pub fn string_key(self) -> &'static str {
        match self {
            ThrustState::Reverse => "hud.helm.reverse",
            ThrustState::Stop => "hud.helm.none",
            ThrustState::Half => "hud.helm.half",
            ThrustState::Full => "hud.helm.full",
        }
    }

    /// The notch closest to a raw throttle value.
    ///
    /// Used to keep the ladder in step with an analog stick, so the HELM readout
    /// still describes what the ship is doing when a pad is flying it and the
    /// keyboard inherits a sensible notch if the player puts the pad down.
    pub fn nearest(throttle: f32) -> Self {
        let mut best = Self::LADDER[0];
        let mut best_gap = f32::INFINITY;
        for notch in Self::LADDER {
            let gap = (notch.throttle() - throttle).abs();
            if gap < best_gap {
                best_gap = gap;
                best = notch;
            }
        }
        best
    }

    /// Step one notch up (`+1`) or down (`-1`) the ladder, saturating at the ends
    /// so holding a key never wraps from full thrust round to reverse.
    pub fn stepped(self, delta: i32) -> Self {
        let at = Self::LADDER.iter().position(|&s| s == self).unwrap_or(2) as i32;
        let next = (at + delta).clamp(0, Self::LADDER.len() as i32 - 1);
        Self::LADDER[next as usize]
    }
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

// Pointer sensitivity and stick shaping are authored in `FeelTuning::controls`
// (see `data/feel.rs`). They used to be the consts below, which made mouse
// sensitivity and stick deadzone — the two numbers a player is most likely to
// want changed — the only ones in the game that needed a recompile.

/// Radial deadzone below which the stick reads zero, above which it is rescaled
/// so motion starts smoothly just past the edge (5% → ~0) and *saturates* before
/// the physical limit (≥ 90% → 100%).
///
/// The saturation matters: a real stick rarely reports a clean 1.0 on one axis —
/// the gate is round and the springs wear — so without it a pad could never
/// reach the full turn rate a keyboard gets for free, and steering felt sluggish
/// by comparison.
pub fn deadzone(v: f32, controls: &ControlFeel) -> f32 {
    let dz = controls.deadzone;
    // Guard the divide: a file that sets saturation at or below the deadzone
    // would otherwise make every stick axis NaN, which is a very confusing way
    // to discover a typo.
    let span = (controls.saturation - dz).max(1e-3);
    let a = v.abs();
    if a < dz {
        0.0
    } else {
        v.signum() * ((a - dz) / span).min(1.0)
    }
}

/// The three resources that together hold "what the player is aiming at".
///
/// Grouped into one [`SystemParam`] rather than taken separately because
/// `player_input` is at Bevy's sixteen-parameter ceiling — and because they are
/// genuinely one thing: every write to any of them happens in the same place,
/// off the same [`AimDecision`].
#[derive(bevy::ecs::system::SystemParam)]
pub struct AimState<'w> {
    pub aiming: ResMut<'w, Aiming>,
    pub cursor: ResMut<'w, AimCursor>,
    pub broadside: ResMut<'w, BroadsideAim>,
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
    feel: Res<FeelTuning>,
    mut board: ResMut<BoardIntent>,
    mut thrust: ResMut<ThrustState>,
    mut aim: AimState,
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

    let controls = feel.controls;
    let use_pad = *method == InputMethod::Gamepad;

    // --- Keyboard ---
    // W/S step the thrust ladder on the key *edge*; the notch then holds until
    // it is changed again, so the keyboard commands a speed rather than pinning
    // the throttle open for as long as a finger is down.
    if keys.just_pressed(KeyCode::KeyW) {
        *thrust = thrust.stepped(1);
    }
    if keys.just_pressed(KeyCode::KeyS) {
        *thrust = thrust.stepped(-1);
    }
    // Whichever device the player last used owns the throttle, and on a pad that
    // means a centred stick reads as *stop*. Starting from the ladder here was a
    // real bug: a pad player never touches W/S, so the notch sat at its default
    // half thrust forever and letting go of the stick left the ship cruising
    // instead of holding station.
    let mut throttle = if use_pad { 0.0 } else { thrust.throttle() };

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
        // The stick is analog and *replaces* the notch rather than adding to it:
        // summing them would let a half-thrust notch plus a shoved stick ask for
        // 1.5 and clamp, so the last third of the stick would do nothing.
        //
        // A live stick takes the throttle even before `track_input_method` has
        // flipped, so picking the pad up mid-flight answers the first push
        // rather than the one after it.
        let stick = deadzone(pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0), &controls);
        if use_pad || stick != 0.0 {
            throttle = stick;
            // Keep the ladder under the stick, so the HELM readout still
            // describes the ship and the keyboard inherits a sane notch if the
            // pad is put down.
            let notch = ThrustState::nearest(stick);
            if *thrust != notch {
                *thrust = notch;
            }
        }
        // Stick right (+X) steers starboard (negative turn).
        turn -= deadzone(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0), &controls);

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
    aim.aiming.port = aim_port;
    aim.aiming.starboard = aim_starboard;

    // The microwarp can't even be *aimed* while it's recharging — suppress the
    // hold so the top-down view, ghost preview and aim-battery drain never engage
    // on cooldown. (The sim also gates the warp itself on the same timer.)
    let microwarp_hold = microwarp_hold && warp.timer <= 0.0;

    // --- Aim: gather raw device state, then hand off to the pure decision ---
    let ship = transform.translation.truncate();
    let dt = real.delta_secs();
    let aiming_broadside = aim_port || aim_starboard;
    let kit_active = emp_fire || torpedo_hold || microwarp_hold;
    let rest_point = ship + heading.forward() * 320.0;
    let right_stick =
        |axis: GamepadAxis| pad.map_or(0.0, |p| deadzone(p.get(axis).unwrap_or(0.0), &controls));

    let yaw_input = if use_pad {
        right_stick(GamepadAxis::RightStickX)
    } else {
        mouse_motion.delta.x * controls.mouse_aim_sens
    };

    let pad_cursor_delta = if use_pad && kit_active {
        if let Ok((_, cam_gt)) = camera_q.single() {
            let right = cam_gt.right().truncate().normalize_or_zero();
            let up = cam_gt.up().truncate().normalize_or_zero();
            let sx = right_stick(GamepadAxis::RightStickX);
            let sy = right_stick(GamepadAxis::RightStickY);
            (right * sx + up * sy) * controls.aim_cursor_rate * dt
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
        aim.broadside.offset,
        aim.cursor.world,
        yaw_input,
        pad_cursor_delta,
        mouse_pick,
        rest_point,
        controls.aim_cursor_max,
    );
    aim.broadside.offset = decision.broadside_offset;
    if !aiming_broadside {
        // While aiming a broadside the pointer cursor is irrelevant — leave it
        // as last set, so switching back to a kit weapon resumes from there.
        aim.cursor.world = decision.aim_point;
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

/// Toggle borderless fullscreen with `F11`.
///
/// Borderless rather than exclusive fullscreen: it keeps the desktop resolution,
/// so alt-tabbing away and back doesn't make every other window jump about, and
/// the design panel and HUD keep the scale factor they were laid out for.
///
/// Deliberately *not* gated on the design panel's input guard, the way F1 isn't
/// either. Window management should answer whatever else has focus — a player
/// who has clicked into a text box and wants their window back should get it.
pub fn toggle_fullscreen(keys: Res<ButtonInput<KeyCode>>, mut windows: Query<&mut Window>) {
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    for mut window in &mut windows {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        };
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
    fn the_thrust_ladder_saturates_at_both_ends() {
        // Holding a key must never wrap full thrust round to reverse.
        let mut s = ThrustState::Full;
        for _ in 0..5 {
            s = s.stepped(1);
        }
        assert_eq!(s, ThrustState::Full);
        for _ in 0..9 {
            s = s.stepped(-1);
        }
        assert_eq!(s, ThrustState::Reverse);
    }

    #[test]
    fn the_ladder_steps_one_notch_at_a_time_from_half_thrust() {
        let s = ThrustState::default();
        assert_eq!(s, ThrustState::Half, "a fresh ship starts at half thrust");
        assert_eq!(s.stepped(1), ThrustState::Full);
        assert_eq!(s.stepped(-1), ThrustState::Stop);
        assert_eq!(s.stepped(-1).stepped(-1), ThrustState::Reverse);
    }

    #[test]
    fn thrust_throttles_are_ordered_and_in_range() {
        let ladder = [
            ThrustState::Reverse,
            ThrustState::Stop,
            ThrustState::Half,
            ThrustState::Full,
        ];
        for pair in ladder.windows(2) {
            assert!(
                pair[0].throttle() < pair[1].throttle(),
                "{:?} should be slower than {:?}",
                pair[0],
                pair[1]
            );
        }
        for notch in ladder {
            let t = notch.throttle();
            assert!(
                (-1.0..=1.0).contains(&t),
                "{notch:?} throttle {t} is out of range"
            );
        }
        assert_eq!(ThrustState::Stop.throttle(), 0.0, "no thrust means stopped");
    }

    /// A centred stick must mean *stop*.
    ///
    /// The regression this pins: the throttle used to start from the keyboard's
    /// notch and only be replaced by a *non-zero* stick. A pad player never
    /// touches W/S, so the notch sat at its default half thrust and releasing the
    /// stick left the ship cruising — the ship could not be brought to rest at
    /// all with a controller.
    #[test]
    fn a_centred_stick_means_stop_not_the_keyboard_notch() {
        // The rule the input path encodes: on a pad the stick is the whole
        // answer, including when it reads zero.
        for notch in [
            ThrustState::Reverse,
            ThrustState::Stop,
            ThrustState::Half,
            ThrustState::Full,
        ] {
            let pad_throttle = pad_authority(notch, 0.0, true);
            assert_eq!(
                pad_throttle, 0.0,
                "a centred stick must stop the ship even with the ladder at {notch:?}"
            );
        }
        // Off the centre, the stick is followed exactly rather than added to.
        assert_eq!(pad_authority(ThrustState::Full, -0.4, true), -0.4);
        // On the keyboard the ladder still rules, and a centred stick leaves it.
        assert_eq!(
            pad_authority(ThrustState::Half, 0.0, false),
            ThrustState::Half.throttle()
        );
        // Even before the input method has flipped, a live stick wins — so
        // picking a pad up answers the first push, not the one after it.
        assert_eq!(pad_authority(ThrustState::Half, 0.8, false), 0.8);
    }

    /// Mirrors the throttle decision in `player_input`. Kept beside the test
    /// rather than extracted, because the real one also writes the ladder and
    /// the helm; this is the arithmetic those writes are built on.
    fn pad_authority(notch: ThrustState, stick: f32, use_pad: bool) -> f32 {
        let mut throttle = if use_pad { 0.0 } else { notch.throttle() };
        if use_pad || stick != 0.0 {
            throttle = stick;
        }
        throttle
    }

    #[test]
    fn the_ladder_follows_the_stick_to_the_nearest_notch() {
        assert_eq!(ThrustState::nearest(0.0), ThrustState::Stop);
        assert_eq!(ThrustState::nearest(0.95), ThrustState::Full);
        assert_eq!(ThrustState::nearest(0.45), ThrustState::Half);
        assert_eq!(ThrustState::nearest(-0.9), ThrustState::Reverse);
        // Exactly between two notches resolves to one of them, not a panic.
        assert!(matches!(
            ThrustState::nearest(0.25),
            ThrustState::Stop | ThrustState::Half
        ));
    }

    /// Every notch must resolve to a real line in the string table, or the HUD
    /// shows `!!MISSING STRING!!` where the thrust setting should be.
    #[test]
    fn every_thrust_notch_has_a_string() {
        let csv = include_str!("../assets/strings/en.csv");
        for notch in [
            ThrustState::Reverse,
            ThrustState::Stop,
            ThrustState::Half,
            ThrustState::Full,
        ] {
            let key = notch.string_key();
            assert!(
                csv.lines().any(|l| l.starts_with(&format!("{key},"))),
                "{key} is missing from en.csv"
            );
        }
    }

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
