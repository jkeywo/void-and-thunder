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
//!
//! This is the *whole* brain for an ordinary enemy, and only that. A ship
//! carrying the full player kit needs to weigh six tools against each other
//! rather than run one; that is [`pilot`](crate::pilot), which flies any ship
//! whose controller sets [`AiController::use_abilities`] and reuses
//! [`desired_helm`] for the beam-work half of the job.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_transform::components::Transform;
use std::f32::consts::{FRAC_PI_2, PI};

use crate::components::{AiController, Disabled, Faction, FireOrders, Heading, Helm, Hull, Ship};
use crate::tuning::{AiTuning, SimTuning};
use crate::util::wrap_angle as wrap;

// Defaults for the AI's gains; the live values are the matching [`AiTuning`]
// fields, which is what the systems pass in.

/// Proportional gain turning a heading error (radians) into a helm command.
pub const TURN_GAIN: f32 = 2.5;
/// Throttle while jockeying for a broadside — mostly turning, holding station.
pub const STATION_THROTTLE: f32 = 0.3;
/// Fraction of throttle spilled at a full 180° of heading error, to buy turn
/// rate. Without this an AI at full thrust can only manage a turn radius wider
/// than its own engagement range, so it orbits its target forever and never
/// closes — the same trap a player falls into before learning to cut thrust.
pub const TURN_EASE: f32 = 0.75;

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

    // Pick a heading to steer toward, and the throttle we would like to hold.
    let (desired_heading, cruise) = if fleeing {
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

    // Spill way to turn. A hull answers the helm far better slowly than at full
    // thrust, so the sharper the turn wanted, the more throttle we give up to get
    // it. Fleeing is exempt: running away is worth a wide turn.
    let throttle = if fleeing {
        cruise
    } else {
        let sharpness = (heading_err.abs() / PI).clamp(0.0, 1.0);
        cruise * (1.0 - tuning.turn_ease * sharpness)
    };

    // Fire a beam when the target is within its firing arc — never while fleeing.
    let mut orders = FireOrders::default();
    if in_range && !fleeing {
        orders.port = wrap(rel - FRAC_PI_2).abs() <= ai.fire_arc;
        orders.starboard = wrap(rel + FRAC_PI_2).abs() <= ai.fire_arc;
        // A turreted hull lays its guns on the target. `broadside_direction`
        // still clamps this to the bank's own arc, so a narrow bank behaves
        // exactly as before and only a wide one can actually follow.
        if ai.aim_at_target {
            orders.aim = Some(to_target / dist);
        }
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
        // A ship flying the full kit is the pilot brain's; exactly one system
        // writes any given helm, so there is never a question of which one won.
        if ai.use_abilities {
            continue;
        }
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

    /// The AI has to make the same speed-versus-agility bargain the player does.
    /// At full thrust a hull's turn radius is wider than its own engagement range,
    /// so an AI that never slows just orbits its target forever without closing.
    #[test]
    fn spills_way_to_make_a_hard_turn() {
        let tuning = AiTuning::default();
        // Target dead astern: the sharpest turn there is.
        let (astern, _) = desired_helm(
            Vec2::ZERO,
            0.0,
            1.0,
            Vec2::new(-1000.0, 0.0),
            &ai(),
            &tuning,
        );
        // Same distance, but already lined up.
        let (ahead, _) = desired_helm(Vec2::ZERO, 0.0, 1.0, Vec2::new(1000.0, 0.0), &ai(), &tuning);
        assert!(
            astern.throttle < ahead.throttle * 0.5,
            "a 180 turn should cost most of the throttle: {} vs {}",
            astern.throttle,
            ahead.throttle
        );
        assert!(astern.throttle > 0.0, "it should still be making way");
    }

    /// Running away is worth a wide turn — a fleeing ship wants distance, not
    /// a tight one, so it must not throttle down to come about.
    #[test]
    fn a_fleeing_ship_keeps_full_throttle_through_the_turn() {
        let (helm, orders) = desired_helm(
            Vec2::ZERO,
            0.0,
            0.1, // below flee_hull_frac
            Vec2::new(50.0, 0.0),
            &ai(),
            &AiTuning::default(),
        );
        assert_eq!(helm.throttle, 1.0, "a fleeing ship runs flat out");
        assert!(!orders.port && !orders.starboard, "and holds its fire");
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
}
