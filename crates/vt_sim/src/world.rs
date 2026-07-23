//! The star system playfield — its soft boundary.
//!
//! The system is a disc of radius [`SystemBounds::radius`] centred on the origin
//! (the star). There is no wall: a ship that strays past the edge feels an
//! inward spring that grows with how far out it is, turning it back toward the
//! action. The force maths is the pure [`bounds_return`] so it is unit-testable.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{Ship, Velocity};

/// The bounds of the current star system: a disc of this radius around origin.
#[derive(Resource, Clone, Copy, Debug)]
pub struct SystemBounds {
    pub radius: f32,
}

impl Default for SystemBounds {
    fn default() -> Self {
        Self { radius: 1400.0 }
    }
}

/// Spring strength pulling a ship back inside the bounds (per unit of overshoot).
const BOUNDS_SPRING: f32 = 3.0;

/// Return the velocity for a ship at `pos`, applying the soft inward spring when
/// it is beyond `radius`. Inside the bounds the velocity is unchanged.
pub fn bounds_return(pos: Vec2, vel: Vec2, radius: f32, dt: f32) -> Vec2 {
    let dist = pos.length();
    if dist <= radius {
        return vel;
    }
    let inward = (-pos).normalize_or_zero();
    let overshoot = dist - radius;
    vel + inward * (overshoot * BOUNDS_SPRING * dt)
}

/// Bevy system: keep ships inside the system with the soft boundary spring.
pub fn bounds_system(
    time: Res<Time>,
    bounds: Res<SystemBounds>,
    mut ships: Query<(&Transform, &mut Velocity), With<Ship>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (transform, mut velocity) in &mut ships {
        let pos = transform.translation.truncate();
        velocity.0 = bounds_return(pos, velocity.0, bounds.radius, dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_the_bounds_velocity_is_unchanged() {
        let v = Vec2::new(100.0, -40.0);
        assert_eq!(bounds_return(Vec2::new(200.0, 0.0), v, 1400.0, 0.1), v);
    }

    #[test]
    fn outside_the_bounds_the_ship_is_pulled_inward() {
        // Far out on +X, coasting further out: velocity must gain an inward (-X) push.
        let out = bounds_return(Vec2::new(2000.0, 0.0), Vec2::new(100.0, 0.0), 1400.0, 0.1);
        assert!(out.x < 100.0, "should be pushed inward, x was {}", out.x);
    }

    #[test]
    fn the_spring_grows_with_overshoot() {
        let near = bounds_return(Vec2::new(1500.0, 0.0), Vec2::ZERO, 1400.0, 0.1);
        let far = bounds_return(Vec2::new(2400.0, 0.0), Vec2::ZERO, 1400.0, 0.1);
        assert!(
            far.x < near.x,
            "further out should pull harder: near={}, far={}",
            near.x,
            far.x
        );
    }
}
