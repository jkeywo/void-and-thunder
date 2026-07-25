//! Enemy ship AI — the M1 milestone.
//!
//! A ship carrying an [`AiController`] steers itself. Each step the AI picks the
//! nearest hostile ship and decides how to behave:
//!
//! - **Flee** — hull below the controller's threshold: point the bow away and
//!   run at full throttle.
//! - **Pursue** — target further than `engage_range`: point the bow at it and
//!   close.
//! - **Present a broadside** — inside `engage_range`: turn so the target sits on
//!   a beam (±90° off the bow), Black-Flag style, and fire that side when the
//!   arc lines up.
//!
//! The decision is the pure function [`desired_helm`] so it is unit-testable
//! without a `World`; [`ai_system`] just finds targets and applies it.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;
use std::f32::consts::{FRAC_PI_2, PI};

use crate::components::{
    AiController, Disabled, Faction, FireOrders, Heading, Helm, Hull, PilotIntent, Ship,
};
use crate::drive::MicrowarpDrive;
use crate::emp::EmpWeapon;
use crate::piracy::BoardIntent;
use crate::torpedo::TorpedoLock;
use crate::tuning::{AiTuning, SimTuning};
use crate::util::wrap_angle as wrap;

// Defaults for the AI's gains; the live values are the matching [`AiTuning`]
// fields, which is what the systems pass in.

/// Proportional gain turning a heading error (radians) into a helm command.
pub const TURN_GAIN: f32 = 2.5;
/// Throttle while jockeying for a broadside — mostly turning, holding station.
pub const STATION_THROTTLE: f32 = 0.3;

/// Decide the [`Helm`] and [`FireOrders`] for one AI ship against one target.
///
/// `hull_frac` is the ship's current hull as a fraction of its max (`0.0..=1.0`).
pub fn desired_helm(
    self_pos: Vec2,
    heading: f32,
    hull_frac: f32,
    target_pos: Vec2,
    ai: &AiController,
    tuning: &AiTuning,
) -> (Helm, FireOrders) {
    let to_target = target_pos - self_pos;
    let dist = to_target.length();
    if dist < 1e-3 {
        return (Helm::default(), FireOrders::default());
    }

    let bearing = to_target.to_angle();
    // Where the target sits relative to our bow: +rel = to port (left).
    let rel = wrap(bearing - heading);

    let fleeing = hull_frac < ai.flee_hull_frac;
    let in_range = dist <= ai.engage_range;

    // Pick a heading to steer toward, and a throttle.
    let (desired_heading, throttle) = if fleeing {
        // Bow away from the threat, run.
        (wrap(bearing + PI), 1.0)
    } else if in_range {
        // Present the nearer beam: put the target at ±90° off the bow.
        let desired = if rel >= 0.0 {
            bearing - FRAC_PI_2 // target to port -> want it on the port beam
        } else {
            bearing + FRAC_PI_2 // target to starboard -> starboard beam
        };
        (wrap(desired), tuning.station_throttle)
    } else {
        // Close the distance, bow on the target.
        (bearing, 1.0)
    };

    let heading_err = wrap(desired_heading - heading);
    let turn = (heading_err * tuning.turn_gain).clamp(-1.0, 1.0);

    // Fire a beam when the target is within its firing arc — never while fleeing.
    let mut orders = FireOrders::default();
    if in_range && !fleeing {
        orders.port = wrap(rel - FRAC_PI_2).abs() <= ai.fire_arc;
        orders.starboard = wrap(rel + FRAC_PI_2).abs() <= ai.fire_arc;
    }

    (Helm { throttle, turn }, orders)
}

/// Bevy system: drive every AI ship toward the nearest hostile ship.
pub fn ai_system(
    tuning: Res<SimTuning>,
    mut controlled: Query<
        (
            &Transform,
            &Heading,
            &Hull,
            &Faction,
            &AiController,
            &mut Helm,
            &mut FireOrders,
        ),
        (With<Ship>, Without<Disabled>),
    >,
    // Crippled ships are prizes, not threats — never target them, so an AI never
    // wastes a broadside finishing off a hulk it (or the player-AI) means to loot.
    targets: Query<(&Transform, &Faction), (With<Ship>, Without<Disabled>)>,
) {
    // Snapshot every ship's position + faction once, so the mutable pass below
    // doesn't conflict with reading potential targets.
    let all: Vec<(Vec2, Faction)> = targets
        .iter()
        .map(|(tf, f)| (tf.translation.truncate(), *f))
        .collect();

    for (transform, heading, hull, faction, ai, mut helm, mut orders) in &mut controlled {
        let self_pos = transform.translation.truncate();

        // Nearest hostile ship.
        let nearest = all
            .iter()
            .filter(|(_, f)| faction.hostile_to(*f))
            .min_by(|a, b| {
                a.0.distance_squared(self_pos)
                    .total_cmp(&b.0.distance_squared(self_pos))
            });

        let Some((target_pos, _)) = nearest else {
            // No enemy in the system: hold station.
            *helm = Helm::default();
            *orders = FireOrders::default();
            continue;
        };

        let hull_frac = (hull.current / hull.max).clamp(0.0, 1.0);
        let (new_helm, new_orders) =
            desired_helm(self_pos, heading.0, hull_frac, *target_pos, ai, &tuning.ai);
        *helm = new_helm;
        *orders = new_orders;
    }
}

/// Enemies within this radius count toward being "surrounded".
pub const SURROUND_RADIUS: f32 = 400.0;
/// This many nearby hostiles triggers a microwarp escape.
pub const SURROUND_COUNT: usize = 2;
/// Seconds the AI holds a microwarp to prime it before releasing (warping).
pub const WARP_PRIME: f32 = 0.4;
/// Torpedo locks the AI builds before releasing a volley.
pub const TORPEDO_MIN_VOLLEY: u32 = 3;

/// What the special-abilities AI wants to do this frame, including a helm that
/// points the bow at the aim point (EMP is a frontal weapon; microwarp aims at
/// the escape point, so the same "face the aim" rule sends the ship the right
/// way in every mode).
pub struct AbilityIntent {
    pub aim_point: Vec2,
    pub helm: Helm,
    /// `Some(helm)` when the pilot should override the broadside-AI's helm to
    /// fight bow-on / steer toward a prize; `None` when there's nothing to
    /// react to (`helm` is then a meaningless default), so the caller should
    /// leave the regular helm standing.
    pub helm_override: Option<Helm>,
    pub emp_fire: bool,
    pub torpedo_hold: bool,
    pub microwarp_hold: bool,
    pub board: bool,
    pub warp_prime: f32,
}

/// A helm that turns the bow toward `aim` at the given throttle.
fn face(ship: Vec2, heading: f32, aim: Vec2, throttle: f32) -> Helm {
    let err = wrap((aim - ship).to_angle() - heading);
    Helm {
        throttle,
        turn: (err * 2.5).clamp(-1.0, 1.0),
    }
}

/// Decide the special-ability intent (pure, so it is unit-testable). Priority:
///
/// - **Microwarp** when surrounded (≥ [`SURROUND_COUNT`] active hostiles within
///   [`SURROUND_RADIUS`]) — warp as far as possible *away* from their centroid.
/// - **EMP** when the nearest active hostile is within EMP range (targetable).
/// - **Board** a crippled ship when no hostile is near — close on it and board.
/// - **Torpedoes** when the only hostile is beyond EMP range and nothing is
///   boardable — hold to build a volley, release once enough tubes are locked.
///
/// `hostiles` are the *active* enemies; `boardable` are crippled (disabled) ships.
#[allow(clippy::too_many_arguments)]
pub fn decide_abilities(
    ship: Vec2,
    heading: f32,
    hostiles: &[Vec2],
    boardable: &[Vec2],
    emp_range: f32,
    microwarp_range: f32,
    microwarp_ready: bool,
    board_range: f32,
    torpedo_locks: u32,
    warp_prime: f32,
    tuning: &AiTuning,
    dt: f32,
) -> AbilityIntent {
    let forward = Vec2::from_angle(heading);
    // Whether there's anything to react to at all — if not, `helm` below stays
    // a meaningless default and must not override the broadside-AI's helm.
    let has_target = !hostiles.is_empty() || !boardable.is_empty();
    let mut out = AbilityIntent {
        aim_point: ship + forward * 300.0,
        helm: Helm::default(),
        helm_override: None,
        emp_fire: false,
        torpedo_hold: false,
        microwarp_hold: false,
        board: false,
        warp_prime: 0.0,
    };
    let nearest = |ships: &[Vec2]| -> Option<Vec2> {
        ships.iter().copied().min_by(|a, b| {
            a.distance_squared(ship)
                .total_cmp(&b.distance_squared(ship))
        })
    };

    // Surrounded by active hostiles -> microwarp escape.
    let near: Vec<Vec2> = hostiles
        .iter()
        .copied()
        .filter(|h| h.distance(ship) < tuning.surround_radius)
        .collect();
    if near.len() >= tuning.surround_count as usize && microwarp_ready {
        let centroid = near.iter().copied().fold(Vec2::ZERO, |a, b| a + b) / near.len() as f32;
        let away = (ship - centroid).normalize_or(forward);
        out.aim_point = ship + away * microwarp_range;
        out.helm = face(ship, heading, out.aim_point, 1.0);
        out.helm_override = Some(out.helm);
        let wp = warp_prime + dt;
        if wp < tuning.warp_prime {
            out.microwarp_hold = true;
            out.warp_prime = wp;
        }
        // Once primed, holding drops to false and prime resets to 0 — the falling
        // edge fires the warp in `microwarp_system`.
        return out;
    }

    // A hostile within EMP range -> EMP it, bow-on.
    if let Some(target) = nearest(hostiles) {
        if target.distance(ship) <= emp_range {
            out.aim_point = target;
            out.emp_fire = true; // the emitter gates arc/target itself
            out.helm = face(ship, heading, target, 0.35);
            out.helm_override = Some(out.helm);
            return out;
        }
    }

    // No hostile near -> go loot the nearest crippled ship. Once alongside, cut
    // the throttle and hold station so the boarding dwell can build.
    if let Some(prize) = nearest(boardable) {
        out.aim_point = prize;
        let dist = prize.distance(ship);
        let throttle = if dist <= board_range {
            0.0
        } else {
            (dist / 300.0).clamp(0.15, 0.6)
        };
        out.helm = face(ship, heading, prize, throttle);
        out.helm_override = Some(out.helm);
        if dist <= board_range {
            out.board = true;
        }
        return out;
    }

    // Otherwise lob torpedoes at the distant hostile.
    if let Some(target) = nearest(hostiles) {
        out.aim_point = target;
        out.torpedo_hold = torpedo_locks < tuning.torpedo_min_volley;
        out.helm = face(ship, heading, target, 0.2);
    }
    out.helm_override = has_target.then_some(out.helm);
    out
}

/// Bevy system: for AI ships with `use_abilities`, drive the special kit through
/// their [`PilotIntent`]. Ships without it (enemies) are untouched.
#[allow(clippy::type_complexity)]
pub fn ai_abilities_system(
    time: Res<Time>,
    tuning: Res<SimTuning>,
    mut board_intent: ResMut<BoardIntent>,
    mut ships: Query<(
        &Transform,
        &Heading,
        &Faction,
        &mut AiController,
        &EmpWeapon,
        &TorpedoLock,
        &MicrowarpDrive,
        &mut PilotIntent,
        &mut Helm,
    )>,
    targets: Query<(&Transform, &Faction, Has<Disabled>), With<Ship>>,
) {
    let dt = time.delta_secs();
    let all: Vec<(Vec2, Faction, bool)> = targets
        .iter()
        .map(|(t, f, disabled)| (t.translation.truncate(), *f, disabled))
        .collect();

    for (transform, heading, faction, mut ai, emp, lock, warp, mut pilot, mut helm) in &mut ships {
        if !ai.use_abilities {
            continue;
        }
        let ship = transform.translation.truncate();
        // Active enemies to fight vs. crippled ships to loot.
        let hostiles: Vec<Vec2> = all
            .iter()
            .filter(|(_, f, disabled)| !disabled && faction.hostile_to(*f))
            .map(|(p, _, _)| *p)
            .collect();
        let boardable: Vec<Vec2> = all
            .iter()
            .filter(|(_, f, disabled)| *disabled && faction.hostile_to(*f))
            .map(|(p, _, _)| *p)
            .collect();

        let d = decide_abilities(
            ship,
            heading.0,
            &hostiles,
            &boardable,
            emp.range,
            warp.range,
            warp.timer <= 0.0,
            tuning.board_range,
            lock.locks,
            ai.warp_prime,
            &tuning.ai,
            dt,
        );
        ai.warp_prime = d.warp_prime;
        pilot.aim_point = d.aim_point;
        pilot.emp_fire = d.emp_fire;
        pilot.torpedo_hold = d.torpedo_hold;
        pilot.microwarp_hold = d.microwarp_hold;
        if d.board {
            board_intent.active = true;
        }
        // Override the broadside helm: this pilot fights bow-on with the kit and
        // steers itself toward prizes. `decide_abilities` itself decides whether
        // there's anything to react to (`None` when not, leaving the regular
        // broadside-AI's helm standing).
        if let Some(h) = d.helm_override {
            *helm = h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ai() -> AiController {
        AiController {
            engage_range: 300.0,
            fire_arc: 0.35,
            flee_hull_frac: 0.25,
            ..Default::default()
        }
    }

    #[test]
    fn pursues_a_distant_target_bow_on() {
        // Target far away on +X, we face +X: close at full throttle, no turn, hold fire.
        let (helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            1.0,
            Vec2::new(1000.0, 0.0),
            &ai(),
            &AiTuning::default(),
        );
        assert_eq!(helm.throttle, 1.0);
        assert!(helm.turn.abs() < 1e-3, "turn was {}", helm.turn);
        assert!(!orders.port && !orders.starboard);
    }

    #[test]
    fn fires_port_when_target_is_on_the_port_beam() {
        // Close target directly to port (+Y) while facing +X.
        let (_helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            1.0,
            Vec2::new(0.0, 100.0),
            &ai(),
            &AiTuning::default(),
        );
        assert!(orders.port, "should fire port");
        assert!(!orders.starboard, "should not fire starboard");
    }

    #[test]
    fn fires_starboard_when_target_is_on_the_starboard_beam() {
        // Close target directly to starboard (-Y) while facing +X.
        let (_helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            1.0,
            Vec2::new(0.0, -100.0),
            &ai(),
            &AiTuning::default(),
        );
        assert!(orders.starboard, "should fire starboard");
        assert!(!orders.port, "should not fire port");
    }

    #[test]
    fn presents_a_beam_when_target_is_close_ahead() {
        // Close target dead ahead: break off (turn) and slow, don't ram.
        let (helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            1.0,
            Vec2::new(100.0, 0.0),
            &ai(),
            &AiTuning::default(),
        );
        assert!(
            helm.turn.abs() > 0.5,
            "should turn to present a beam, turn={}",
            helm.turn
        );
        assert!(
            helm.throttle < 1.0,
            "should not charge, throttle={}",
            helm.throttle
        );
        assert!(!orders.port && !orders.starboard, "beam not lined up yet");
    }

    #[test]
    fn flees_when_hull_is_low() {
        // Crippled, target dead ahead: turn hard away and run, hold fire.
        let (helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            0.1,
            Vec2::new(100.0, 0.0),
            &ai(),
            &AiTuning::default(),
        );
        assert_eq!(helm.throttle, 1.0);
        assert!(
            helm.turn.abs() > 0.9,
            "should turn hard away, turn={}",
            helm.turn
        );
        assert!(
            !orders.port && !orders.starboard,
            "must not fire while fleeing"
        );
    }

    /// Run the real `ai_system` through an ECS `Schedule` (no window, no Time):
    /// an AI House ship must target the enemy player and drive its own helm/fire
    /// orders — proving the system's queries wire up and don't conflict.
    #[test]
    fn ai_system_drives_a_ship_at_its_enemy() {
        use crate::components::Hull;
        use bevy_ecs::prelude::*;

        let mut world = World::new();

        // Player (Corsairs) at origin — no AiController, so the AI never drives it.
        world.spawn((
            Ship,
            Faction::Corsairs,
            Transform::from_xyz(0.0, 0.0, 0.0),
            Heading(0.0),
            Hull::new(100.0),
            Helm::default(),
            FireOrders::default(),
        ));

        // House ship at (0, 100) facing +X: the player lies off its starboard
        // beam and within engage range, so it should fire starboard.
        let enemy = world
            .spawn((
                Ship,
                Faction::Houses,
                AiController::default(),
                Transform::from_xyz(0.0, 100.0, 0.0),
                Heading(0.0),
                Hull::new(100.0),
                Helm::default(),
                FireOrders::default(),
            ))
            .id();

        world.insert_resource(SimTuning::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(ai_system);
        schedule.run(&mut world);

        let orders = world.get::<FireOrders>(enemy).unwrap();
        assert!(orders.starboard, "AI House ship should fire on the player");
        let helm = world.get::<Helm>(enemy).unwrap();
        assert!(
            helm.throttle != 0.0 || helm.turn != 0.0,
            "AI should have set a non-idle helm"
        );
    }

    /// An AI ship must not fire on a *crippled* hostile sitting on its beam — a
    /// hulk is loot, not a target. Without excluding `Disabled` ships from the
    /// AI's target set, the player-AI would destroy the ship it means to board.
    #[test]
    fn ai_holds_fire_on_a_crippled_prize() {
        use crate::components::Hull;
        use bevy_ecs::prelude::*;

        let mut world = World::new();

        // A crippled Corsair dead on the House ship's port beam (+Y), in range.
        world.spawn((
            Ship,
            Faction::Corsairs,
            Disabled,
            Transform::from_xyz(0.0, 100.0, 0.0),
            Heading(0.0),
            Hull {
                current: 10.0,
                max: 100.0,
            },
            Helm::default(),
            FireOrders::default(),
        ));

        // House ship at the origin facing +X — the only hostile is the hulk.
        let enemy = world
            .spawn((
                Ship,
                Faction::Houses,
                AiController::default(),
                Transform::from_xyz(0.0, 0.0, 0.0),
                Heading(0.0),
                Hull::new(100.0),
                Helm::default(),
                FireOrders::default(),
            ))
            .id();

        world.insert_resource(SimTuning::default());
        let mut schedule = Schedule::default();
        schedule.add_systems(ai_system);
        schedule.run(&mut world);

        let orders = world.get::<FireOrders>(enemy).unwrap();
        assert!(
            !orders.port && !orders.starboard,
            "AI must not fire on a crippled prize"
        );
    }

    #[test]
    fn emp_when_a_target_is_in_range() {
        // One hostile within EMP range → fire EMP, no torpedoes.
        let d = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[Vec2::new(300.0, 0.0)],
            &[],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(d.emp_fire, "should EMP a targetable ship");
        assert!(!d.torpedo_hold && !d.microwarp_hold);
    }

    #[test]
    fn torpedoes_when_the_only_target_is_far() {
        // Nearest hostile beyond EMP range, none closer → build a torpedo volley.
        let d = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[Vec2::new(1000.0, 0.0)],
            &[],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(!d.emp_fire, "target is too far for EMP");
        assert!(d.torpedo_hold, "should hold torpedoes to lock the volley");
        // Once enough tubes are locked, it releases (hold=false) to fire.
        let released = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[Vec2::new(1000.0, 0.0)],
            &[],
            620.0,
            900.0,
            true,
            95.0,
            5,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(
            !released.torpedo_hold,
            "should release once enough are locked"
        );
    }

    #[test]
    fn microwarp_away_when_surrounded() {
        // Three hostiles clustered on +X → warp toward -X (away), at max range.
        let hostiles = [
            Vec2::new(200.0, 20.0),
            Vec2::new(220.0, -30.0),
            Vec2::new(180.0, 0.0),
        ];
        let d = decide_abilities(
            Vec2::ZERO,
            0.0,
            &hostiles,
            &[],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(d.microwarp_hold, "should prime a microwarp");
        assert!(
            d.aim_point.x < 0.0,
            "should warp away from the enemies (−X)"
        );
        assert!(
            (d.aim_point.length() - 900.0).abs() < 1.0,
            "should warp at max range"
        );
    }

    #[test]
    fn boards_a_crippled_ship_when_no_hostile_near() {
        // No active hostiles; a crippled ship within board range → board it.
        let d = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[],
            &[Vec2::new(50.0, 0.0)],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(d.board, "should board a crippled ship in range");
        // A crippled ship out of range → close on it, don't board yet.
        let far = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[],
            &[Vec2::new(400.0, 0.0)],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(!far.board, "too far to board yet");
        assert!(far.helm.throttle > 0.0, "should close on the prize");
    }

    #[test]
    fn decide_abilities_overrides_helm_only_when_hostiles_or_prizes_exist() {
        // Nothing to react to — the caller must not override the broadside-AI's helm.
        let nothing = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[],
            &[],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(nothing.helm_override.is_none());

        // A hostile in range — the kit takes over steering.
        let hostile = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[Vec2::new(300.0, 0.0)],
            &[],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(hostile.helm_override.is_some());

        // A crippled prize with no active hostile — still overrides to steer toward it.
        let prize = decide_abilities(
            Vec2::ZERO,
            0.0,
            &[],
            &[Vec2::new(400.0, 0.0)],
            620.0,
            900.0,
            true,
            95.0,
            0,
            0.0,
            &AiTuning::default(),
            0.1,
        );
        assert!(prize.helm_override.is_some());
    }
}
