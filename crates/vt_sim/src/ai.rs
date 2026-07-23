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
use bevy_transform::components::Transform;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::components::{AiController, Disabled, Faction, FireOrders, Heading, Helm, Hull, Ship};

/// Wrap an angle to the range `(-PI, PI]`.
fn wrap(angle: f32) -> f32 {
    let a = angle.rem_euclid(TAU);
    if a > PI {
        a - TAU
    } else {
        a
    }
}

/// Proportional gain turning a heading error (radians) into a helm command.
const TURN_GAIN: f32 = 2.5;
/// Throttle while jockeying for a broadside — mostly turning, holding station.
const STATION_THROTTLE: f32 = 0.3;

/// Decide the [`Helm`] and [`FireOrders`] for one AI ship against one target.
///
/// `hull_frac` is the ship's current hull as a fraction of its max (`0.0..=1.0`).
pub fn desired_helm(
    self_pos: Vec2,
    heading: f32,
    hull_frac: f32,
    target_pos: Vec2,
    ai: &AiController,
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
        (wrap(desired), STATION_THROTTLE)
    } else {
        // Close the distance, bow on the target.
        (bearing, 1.0)
    };

    let heading_err = wrap(desired_heading - heading);
    let turn = (heading_err * TURN_GAIN).clamp(-1.0, 1.0);

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
    targets: Query<(&Transform, &Faction), With<Ship>>,
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
        let (new_helm, new_orders) = desired_helm(self_pos, heading.0, hull_frac, *target_pos, ai);
        *helm = new_helm;
        *orders = new_orders;
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
        }
    }

    #[test]
    fn pursues_a_distant_target_bow_on() {
        // Target far away on +X, we face +X: close at full throttle, no turn, hold fire.
        let (helm, orders) = desired_helm(Vec2::ZERO, 0.0, 1.0, Vec2::new(1000.0, 0.0), &ai());
        assert_eq!(helm.throttle, 1.0);
        assert!(helm.turn.abs() < 1e-3, "turn was {}", helm.turn);
        assert!(!orders.port && !orders.starboard);
    }

    #[test]
    fn fires_port_when_target_is_on_the_port_beam() {
        // Close target directly to port (+Y) while facing +X.
        let (_helm, orders) = desired_helm(Vec2::ZERO, 0.0, 1.0, Vec2::new(0.0, 100.0), &ai());
        assert!(orders.port, "should fire port");
        assert!(!orders.starboard, "should not fire starboard");
    }

    #[test]
    fn fires_starboard_when_target_is_on_the_starboard_beam() {
        // Close target directly to starboard (-Y) while facing +X.
        let (_helm, orders) = desired_helm(Vec2::ZERO, 0.0, 1.0, Vec2::new(0.0, -100.0), &ai());
        assert!(orders.starboard, "should fire starboard");
        assert!(!orders.port, "should not fire port");
    }

    #[test]
    fn presents_a_beam_when_target_is_close_ahead() {
        // Close target dead ahead: break off (turn) and slow, don't ram.
        let (helm, orders) = desired_helm(Vec2::ZERO, 0.0, 1.0, Vec2::new(100.0, 0.0), &ai());
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
        let (helm, orders) = desired_helm(Vec2::ZERO, 0.0, 0.1, Vec2::new(100.0, 0.0), &ai());
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
}
