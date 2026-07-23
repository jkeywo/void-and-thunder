//! Ship movement: turning, thrust, drag and integration.
//!
//! The physics is deliberately arcade-y and Black-Flag-flavoured: the helm
//! rotates the hull, thrust pushes along the bow, drag bleeds off speed, and
//! velocity is clamped to a sail-set maximum. The maths lives in [`helm_step`]
//! as a pure function so it can be unit-tested without an ECS `World`; the
//! [`movement_system`] is a thin wrapper that applies it to every ship.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec2};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{Heading, Helm, ShipStats, Velocity};

/// Advance one ship's heading and velocity by `dt` seconds.
///
/// Returns the new `(heading, velocity)`. Pure and deterministic — the same
/// inputs always yield the same output, which is what the tests rely on.
pub fn helm_step(
    heading: f32,
    velocity: Vec2,
    stats: &ShipStats,
    helm: &Helm,
    dt: f32,
) -> (f32, Vec2) {
    let turn = helm.turn.clamp(-1.0, 1.0);
    let throttle = helm.throttle.clamp(-1.0, 1.0);

    // Rotate the hull.
    let new_heading = heading + turn * stats.turn_rate * dt;

    // Thrust along the (new) bow direction, then apply drag, then clamp.
    let forward = Vec2::from_angle(new_heading);
    let mut vel = velocity + forward * (throttle * stats.thrust * dt);
    vel *= 1.0 - (stats.linear_drag * dt).clamp(0.0, 1.0);
    if vel.length() > stats.max_speed {
        vel = vel.normalize_or_zero() * stats.max_speed;
    }

    (new_heading, vel)
}

/// Bevy system: apply [`helm_step`] to every ship and integrate its position.
pub fn movement_system(
    time: Res<Time>,
    mut ships: Query<(
        &mut Transform,
        &mut Heading,
        &mut Velocity,
        &ShipStats,
        &Helm,
    )>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (mut transform, mut heading, mut velocity, stats, helm) in &mut ships {
        let (new_heading, new_velocity) = helm_step(heading.0, velocity.0, stats, helm, dt);
        heading.0 = new_heading;
        velocity.0 = new_velocity;
        transform.translation += (new_velocity * dt).extend(0.0);
        // Keep the rendered transform in sync with the sim's heading.
        transform.rotation = Quat::from_rotation_z(new_heading);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sloop() -> ShipStats {
        ShipStats {
            thrust: 100.0,
            turn_rate: 1.0,
            max_speed: 200.0,
            linear_drag: 0.0,
        }
    }

    #[test]
    fn full_throttle_accelerates_along_the_bow() {
        // Facing +X, one second of full forward thrust => +100 units/s on X.
        let (_h, v) = helm_step(
            0.0,
            Vec2::ZERO,
            &sloop(),
            &Helm {
                throttle: 1.0,
                turn: 0.0,
            },
            1.0,
        );
        assert!((v.x - 100.0).abs() < 1e-4, "vx was {}", v.x);
        assert!(v.y.abs() < 1e-4, "vy was {}", v.y);
    }

    #[test]
    fn helm_turns_the_hull() {
        let (h, _v) = helm_step(
            0.0,
            Vec2::ZERO,
            &sloop(),
            &Helm {
                throttle: 0.0,
                turn: 1.0,
            },
            0.5,
        );
        assert!((h - 0.5).abs() < 1e-4, "heading was {}", h);
    }

    #[test]
    fn speed_is_clamped_to_max() {
        // Accelerate hard for many steps; must never exceed max_speed.
        let mut v = Vec2::ZERO;
        for _ in 0..1000 {
            (_, v) = helm_step(
                0.0,
                v,
                &sloop(),
                &Helm {
                    throttle: 1.0,
                    turn: 0.0,
                },
                0.1,
            );
        }
        assert!(v.length() <= 200.0 + 1e-3, "speed was {}", v.length());
    }

    #[test]
    fn drag_bleeds_off_a_coasting_ship() {
        let stats = ShipStats {
            linear_drag: 0.5,
            ..sloop()
        };
        let (_, v) = helm_step(0.0, Vec2::new(100.0, 0.0), &stats, &Helm::default(), 1.0);
        assert!(
            v.length() < 100.0,
            "coasting speed should drop, was {}",
            v.length()
        );
    }
}
