//! Ship movement: turning, thrust, drag and integration.
//!
//! The physics is deliberately arcade-y and Black-Flag-flavoured. Three things
//! make a hull read as a ship rather than a cursor:
//!
//! - **Speed costs agility.** The helm's turn rate is scaled by [`agility_at`],
//!   which falls off as the ship approaches its top speed. Spilling your way to
//!   pivot and then making sail again is the core manoeuvre.
//! - **The hull has rotational inertia.** The helm asks for a rate of turn and
//!   the ship eases onto it, so even a digital key press leans the hull over.
//! - **Drag is anisotropic.** Low along the bow (the ship carries its way), high
//!   across the beam (it bites sideways instead of skidding).
//!
//! The maths lives in [`helm_step`] as a pure function so it can be unit-tested
//! without an ECS `World`; the [`movement_system`] is a thin wrapper that applies
//! it to every ship.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec2};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{
    Anchored, AngularVelocity, Heading, Helm, ShipStats, SpeedScale, Velocity,
};
use crate::tuning::SimTuning;
use crate::util::wrap_angle;

/// Default fraction of forward thrust available in reverse — backing sails only
/// push a quarter as hard, so reverse tops out at ~25% of forward speed. The live
/// value is [`SimTuning::reverse_throttle`].
pub const REVERSE_THROTTLE: f32 = 0.25;

/// The turn-rate multiplier available at `speed` for a hull with these stats.
///
/// A standstill gives [`ShipStats::turn_rate_slow`], `max_speed` gives
/// [`ShipStats::turn_rate_fast`], and it lerps between. This is the Black-Flag
/// bargain: full sail is fastest and stiffest, and the way to pivot is to spill
/// your way first. Split out as its own function because the client draws a
/// turn-rate readout from it and the tests assert on it directly.
pub fn agility_at(stats: &ShipStats, speed: f32) -> f32 {
    if stats.max_speed <= 0.0 {
        return stats.turn_rate_slow;
    }
    let frac = (speed / stats.max_speed).clamp(0.0, 1.0);
    stats.turn_rate_slow + (stats.turn_rate_fast - stats.turn_rate_slow) * frac
}

/// Advance one ship's heading, rate of turn and velocity by `dt` seconds.
///
/// Returns the new `(heading, angular_velocity, velocity)`. Pure and
/// deterministic — the same inputs always yield the same output, which is what
/// the tests rely on.
pub fn helm_step(
    heading: f32,
    angular_velocity: f32,
    velocity: Vec2,
    stats: &ShipStats,
    helm: &Helm,
    reverse_throttle: f32,
    dt: f32,
) -> (f32, f32, Vec2) {
    let turn = helm.turn.clamp(-1.0, 1.0);
    let throttle = helm.throttle.clamp(-1.0, 1.0);
    // Reverse is weak — a fraction of forward thrust.
    let drive = if throttle < 0.0 {
        throttle * reverse_throttle
    } else {
        throttle
    };

    // Rotate the hull. The helm asks for a rate of turn — scaled down the faster
    // the ship is going — and the hull eases onto it rather than snapping, so a
    // digital key press still reads as a ship leaning over.
    let target_omega = turn * stats.turn_rate * agility_at(stats, velocity.length());
    let omega = angular_velocity
        + (target_omega - angular_velocity) * (1.0 - (-stats.turn_accel * dt).exp());
    let new_heading = heading + omega * dt;

    // Thrust along the (new) bow, then damp the bow and beam components
    // separately. The exponential is exact rather than a per-step approximation,
    // so the result does not drift with the step size.
    let forward = Vec2::from_angle(new_heading);
    let right = Vec2::new(forward.y, -forward.x);
    let vel = velocity + forward * (drive * stats.thrust * dt);
    let along = vel.dot(forward) * (-stats.forward_drag * dt).exp();
    let side = vel.dot(right) * (-stats.lateral_drag * dt).exp();
    let mut vel = forward * along + right * side;
    if vel.length() > stats.max_speed {
        vel = vel.normalize_or_zero() * stats.max_speed;
    }

    (new_heading, omega, vel)
}

/// Bevy system: apply [`helm_step`] to every ship and integrate its position.
///
/// The ship's [`SpeedScale`] (set by the drive systems from EMP + boost) scales
/// its top speed and thrust for this step, so `helm_step` itself stays pure and
/// unaware of the modifiers.
pub fn movement_system(
    time: Res<Time>,
    tuning: Res<SimTuning>,
    // `Anchored` ships never move: skipping them here is what makes a test-range
    // target hold its mark even if something writes its Helm.
    mut ships: Query<
        (
            &mut Transform,
            &mut Heading,
            &mut AngularVelocity,
            &mut Velocity,
            &ShipStats,
            &Helm,
            &SpeedScale,
        ),
        Without<Anchored>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut transform, mut heading, mut omega, mut velocity, stats, helm, scale) in &mut ships {
        // Scaling `max_speed` alongside `thrust` keeps the agility curve honest:
        // a boosting ship is measured against its boosted top speed, so the boost
        // buys speed without quietly handing back the turn rate it should cost.
        let scaled = ShipStats {
            max_speed: stats.max_speed * scale.0,
            thrust: stats.thrust * scale.0,
            ..*stats
        };
        let (new_heading, new_omega, new_velocity) = helm_step(
            heading.0,
            omega.0,
            velocity.0,
            &scaled,
            helm,
            tuning.reverse_throttle,
            dt,
        );
        heading.0 = wrap_angle(new_heading);
        omega.0 = new_omega;
        velocity.0 = new_velocity;
        transform.translation += (new_velocity * dt).extend(0.0);
        // Keep the rendered transform in sync with the sim's heading.
        transform.rotation = Quat::from_rotation_z(new_heading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frictionless test hull with no rotational inertia and a flat agility
    /// curve, so the older tests measure one thing at a time. The tests that care
    /// about drag, agility or inertia override the relevant fields.
    fn sloop() -> ShipStats {
        ShipStats {
            thrust: 100.0,
            turn_rate: 1.0,
            max_speed: 200.0,
            forward_drag: 0.0,
            lateral_drag: 0.0,
            turn_rate_slow: 1.0,
            turn_rate_fast: 1.0,
            turn_accel: f32::INFINITY,
        }
    }

    #[test]
    fn full_throttle_accelerates_along_the_bow() {
        // Facing +X, one second of full forward thrust => +100 units/s on X.
        let (_h, _w, v) = helm_step(
            0.0,
            0.0,
            Vec2::ZERO,
            &sloop(),
            &Helm {
                throttle: 1.0,
                turn: 0.0,
            },
            REVERSE_THROTTLE,
            1.0,
        );
        assert!((v.x - 100.0).abs() < 1e-4, "vx was {}", v.x);
        assert!(v.y.abs() < 1e-4, "vy was {}", v.y);
    }

    #[test]
    fn reverse_is_a_quarter_of_forward_thrust() {
        // Facing +X, one second of full reverse => 25% of the forward impulse.
        let (_h, _w, v) = helm_step(
            0.0,
            0.0,
            Vec2::ZERO,
            &sloop(),
            &Helm {
                throttle: -1.0,
                turn: 0.0,
            },
            REVERSE_THROTTLE,
            1.0,
        );
        assert!(
            (v.x + 25.0).abs() < 1e-4,
            "reverse vx should be -25 (25% of 100), was {}",
            v.x
        );
    }

    #[test]
    fn helm_turns_the_hull() {
        let (h, _w, _v) = helm_step(
            0.0,
            0.0,
            Vec2::ZERO,
            &sloop(),
            &Helm {
                throttle: 0.0,
                turn: 1.0,
            },
            REVERSE_THROTTLE,
            0.5,
        );
        assert!((h - 0.5).abs() < 1e-4, "heading was {}", h);
    }

    #[test]
    fn speed_is_clamped_to_max() {
        // Accelerate hard for many steps; must never exceed max_speed.
        let mut v = Vec2::ZERO;
        for _ in 0..1000 {
            (_, _, v) = helm_step(
                0.0,
                0.0,
                v,
                &sloop(),
                &Helm {
                    throttle: 1.0,
                    turn: 0.0,
                },
                REVERSE_THROTTLE,
                0.1,
            );
        }
        assert!(v.length() <= 200.0 + 1e-3, "speed was {}", v.length());
    }

    #[test]
    fn drag_bleeds_off_a_coasting_ship() {
        let stats = ShipStats {
            forward_drag: 0.5,
            ..sloop()
        };
        let (_, _, v) = helm_step(
            0.0,
            0.0,
            Vec2::new(100.0, 0.0),
            &stats,
            &Helm::default(),
            REVERSE_THROTTLE,
            1.0,
        );
        assert!(
            v.length() < 100.0,
            "coasting speed should drop, was {}",
            v.length()
        );
    }

    #[test]
    fn a_slow_ship_is_handier_than_a_fast_one() {
        let stats = ShipStats::default();
        let at_rest = agility_at(&stats, 0.0);
        let at_speed = agility_at(&stats, stats.max_speed);
        assert!(
            at_rest > at_speed,
            "dropping sail must buy turn rate: {at_rest} at rest vs {at_speed} at speed"
        );
        // The gap is the skill expression; a token difference is not enough to
        // read in play, so hold the shipped hull to a decisive one.
        assert!(
            at_rest > at_speed * 2.0,
            "the speed/agility gap should be dramatic, was {at_rest} vs {at_speed}"
        );
        // Half speed sits between the two ends.
        let mid = agility_at(&stats, stats.max_speed * 0.5);
        assert!(
            mid < at_rest && mid > at_speed,
            "mid-speed agility was {mid}"
        );
    }

    #[test]
    fn the_hull_bites_sideways_harder_than_it_drags_astern() {
        // Facing +X, coasting diagonally: after a step the sideways component
        // must have bled off proportionally more than the forward one.
        let stats = ShipStats::default();
        let (_, _, v) = helm_step(
            0.0,
            0.0,
            Vec2::new(100.0, 100.0),
            &stats,
            &Helm::default(),
            REVERSE_THROTTLE,
            0.25,
        );
        let kept_forward = v.x / 100.0;
        let kept_sideways = v.y.abs() / 100.0;
        assert!(
            kept_forward > kept_sideways,
            "a ship should slide less than it coasts: kept {kept_forward} forward vs {kept_sideways} sideways"
        );
    }

    /// Hold a sail notch until the ship settles, then put the helm hard over and
    /// measure how long a 180° turn takes and how wide it is. This is the
    /// headline claim of the whole handling model, so it is pinned here rather
    /// than left to playtesting.
    fn come_about(throttle: f32) -> (f32, f32) {
        let stats = ShipStats::default();
        let dt = 1.0 / 64.0;
        let (mut heading, mut omega, mut vel) = (0.0f32, 0.0f32, Vec2::ZERO);

        // Settle onto the notch, running straight.
        let straight = Helm {
            throttle,
            turn: 0.0,
        };
        for _ in 0..640 {
            (heading, omega, vel) = helm_step(heading, omega, vel, &stats, &straight, 0.25, dt);
        }

        // Helm hard over until the bow has swung 180°.
        let hard_over = Helm {
            throttle,
            turn: 1.0,
        };
        let start = heading;
        let mut pos = Vec2::ZERO;
        let mut widest: f32 = 0.0;
        let mut secs = 0.0;
        while (heading - start).abs() < std::f32::consts::PI && secs < 60.0 {
            (heading, omega, vel) = helm_step(heading, omega, vel, &stats, &hard_over, 0.25, dt);
            pos += vel * dt;
            widest = widest.max(pos.length());
            secs += dt;
        }
        (secs, widest)
    }

    #[test]
    fn taking_in_sail_buys_a_tighter_turn() {
        let (full_secs, full_width) = come_about(1.0);
        let (half_secs, half_width) = come_about(0.5);
        let (stop_secs, stop_width) = come_about(0.0);
        println!(
            "come about 180deg:\n  full sail: {full_secs:.2}s, {full_width:.0} units wide\n  \
             half sail: {half_secs:.2}s, {half_width:.0} units wide\n  \
             all stop:  {stop_secs:.2}s, {stop_width:.0} units wide"
        );

        assert!(
            stop_width < half_width && half_width < full_width,
            "each notch down the ladder must tighten the turn: \
             {stop_width} / {half_width} / {full_width}"
        );
        // The pivot has to be worth doing, not a rounding error. Coming about at
        // full sail should sweep several times the room an all-stop pivot needs.
        assert!(
            full_width > stop_width * 4.0,
            "the drop-sail pivot should be dramatically tighter: \
             {full_width} at full sail vs {stop_width} stopped"
        );
        assert!(
            stop_secs < full_secs,
            "and quicker: {stop_secs}s stopped vs {full_secs}s at full sail"
        );
    }

    #[test]
    fn rate_of_turn_ramps_rather_than_snapping() {
        let stats = ShipStats::default();
        let hard_over = Helm {
            throttle: 0.0,
            turn: 1.0,
        };
        // One short step from rest gets nowhere near the rate the helm is asking
        // for — the hull has to swing into it.
        let (_, omega, _) = helm_step(
            0.0,
            0.0,
            Vec2::ZERO,
            &stats,
            &hard_over,
            REVERSE_THROTTLE,
            1.0 / 64.0,
        );
        let target = stats.turn_rate * agility_at(&stats, 0.0);
        assert!(
            omega > 0.0 && omega < target * 0.25,
            "one step should only start the swing: {omega} of a {target} target"
        );

        // Held over, it converges on that target.
        let mut omega = 0.0;
        for _ in 0..256 {
            (_, omega, _) = helm_step(
                0.0,
                omega,
                Vec2::ZERO,
                &stats,
                &hard_over,
                REVERSE_THROTTLE,
                1.0 / 64.0,
            );
        }
        assert!(
            (omega - target).abs() < 1e-3,
            "a held helm should reach {target}, got {omega}"
        );

        // Centring the helm bleeds the swing off rather than stopping it dead.
        let (_, coasting, _) = helm_step(
            0.0,
            omega,
            Vec2::ZERO,
            &stats,
            &Helm::default(),
            REVERSE_THROTTLE,
            1.0 / 64.0,
        );
        assert!(
            coasting > 0.0 && coasting < omega,
            "the hull should keep swinging briefly: {coasting} after {omega}"
        );
    }
}
