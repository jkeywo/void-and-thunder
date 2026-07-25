//! Ship drives and per-ship speed scaling.
//!
//! Two systems run each step before movement:
//! - [`battery_system`] recovers EMP damage and drains/recharges boost batteries.
//! - [`speed_scale_system`] folds EMP and boost into each ship's [`SpeedScale`],
//!   which [`movement_system`](crate::ship::movement_system) then applies.
//!
//! The maths that decides the multiplier is the pure [`speed_scale`] so it can be
//! unit-tested without a `World`.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::components::{Anchored, EmpDefense, PilotIntent, Ship, SpeedScale, ENGAGEMENT_RANGE};

/// A rechargeable overdrive. While `active` and `battery > 0`, the ship's speed
/// is multiplied by `multiplier`. Config + live state live together so the
/// client just flips `active`. Player-only for now.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoostDrive {
    pub multiplier: f32,
    pub battery: f32,
    pub battery_max: f32,
    pub drain_per_sec: f32,
    pub recharge_per_sec: f32,
    /// Set by the controller each frame — is the pilot holding boost?
    pub active: bool,
}

impl Default for BoostDrive {
    fn default() -> Self {
        Self {
            multiplier: 1.6,
            battery: 3.0,
            battery_max: 3.0,
            drain_per_sec: 1.0,
            recharge_per_sec: 0.6,
            active: false,
        }
    }
}

impl BoostDrive {
    /// True when boost is actually contributing thrust this frame.
    pub fn engaged(&self) -> bool {
        self.active && self.battery > 0.0
    }
}

/// A short-range teleport drive. Holding aim previews a destination within
/// `range`; releasing warps there (gated by `cooldown`). Player-only.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MicrowarpDrive {
    pub range: f32,
    pub cooldown: f32,
    pub timer: f32,
    pub was_holding: bool,
}

impl Default for MicrowarpDrive {
    fn default() -> Self {
        Self {
            range: ENGAGEMENT_RANGE,
            cooldown: 2.0,
            timer: 0.0,
            was_holding: false,
        }
    }
}

/// The speed multiplier for a ship given its EMP load and (optional) boost.
pub fn speed_scale(emp: &EmpDefense, boost: Option<&BoostDrive>) -> f32 {
    let boost_factor = match boost {
        Some(b) if b.engaged() => b.multiplier,
        _ => 1.0,
    };
    emp.speed_factor() * boost_factor
}

/// Bevy system: recover EMP damage and service boost batteries.
pub fn battery_system(
    time: Res<Time>,
    mut ships: Query<(&mut EmpDefense, Option<&mut BoostDrive>)>,
) {
    let dt = time.delta_secs();
    for (mut emp, boost) in &mut ships {
        emp.damage = (emp.damage - emp.recovery_per_sec * dt).max(0.0);
        if let Some(mut boost) = boost {
            if boost.active && boost.battery > 0.0 {
                boost.battery = (boost.battery - boost.drain_per_sec * dt).max(0.0);
            } else {
                boost.battery =
                    (boost.battery + boost.recharge_per_sec * dt).min(boost.battery_max);
            }
        }
    }
}

/// Bevy system: recompute every ship's [`SpeedScale`].
pub fn speed_scale_system(
    mut ships: Query<(&mut SpeedScale, &EmpDefense, Option<&BoostDrive>), With<Ship>>,
) {
    for (mut scale, emp, boost) in &mut ships {
        scale.0 = speed_scale(emp, boost);
    }
}

/// Clamp `point` to lie within `range` of `origin`.
pub fn clamp_to_range(origin: Vec2, point: Vec2, range: f32) -> Vec2 {
    let delta = point - origin;
    if delta.length() > range {
        origin + delta.normalize_or_zero() * range
    } else {
        point
    }
}

/// Bevy system: teleport a microwarp ship to the aim point (clamped to range)
/// when the pilot releases, if the drive has cooled down. Reads each ship's own
/// [`PilotIntent`], so player and AI ships share the system.
pub fn microwarp_system(
    time: Res<Time>,
    mut ships: Query<(&mut Transform, &mut MicrowarpDrive, &PilotIntent), Without<Anchored>>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut drive, intent) in &mut ships {
        drive.timer = (drive.timer - dt).max(0.0);
        let hold = intent.microwarp_hold;
        if drive.was_holding && !hold && drive.timer <= 0.0 {
            let origin = transform.translation.truncate();
            let dest = clamp_to_range(origin, intent.aim_point, drive.range);
            transform.translation.x = dest.x;
            transform.translation.y = dest.y;
            drive.timer = drive.cooldown;
        }
        drive.was_holding = hold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_ship_runs_at_full_speed() {
        assert!((speed_scale(&EmpDefense::default(), None) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn emp_inverse_lerps_speed() {
        let half = EmpDefense {
            resist: 100.0,
            damage: 50.0,
            recovery_per_sec: 0.0,
        };
        assert!((speed_scale(&half, None) - 0.5).abs() < 1e-6);
        let full = EmpDefense {
            damage: 100.0,
            ..half
        };
        assert!(speed_scale(&full, None).abs() < 1e-6);
    }

    #[test]
    fn boost_multiplies_when_engaged_and_charged() {
        let emp = EmpDefense::default();
        let boost = BoostDrive {
            multiplier: 1.6,
            battery: 1.0,
            active: true,
            ..Default::default()
        };
        assert!((speed_scale(&emp, Some(&boost)) - 1.6).abs() < 1e-6);
        // Flat battery => no boost.
        let empty = BoostDrive {
            battery: 0.0,
            ..boost
        };
        assert!((speed_scale(&emp, Some(&empty)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn emp_and_boost_stack() {
        let emp = EmpDefense {
            resist: 100.0,
            damage: 50.0,
            recovery_per_sec: 0.0,
        };
        let boost = BoostDrive {
            multiplier: 1.6,
            battery: 1.0,
            active: true,
            ..Default::default()
        };
        assert!((speed_scale(&emp, Some(&boost)) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn microwarp_clamps_to_range() {
        // A point 1000 out with a 900 range lands on the edge.
        let dest = clamp_to_range(Vec2::ZERO, Vec2::new(1000.0, 0.0), 900.0);
        assert!((dest.x - 900.0).abs() < 1e-3);
        // A point inside range is unchanged.
        let near = clamp_to_range(Vec2::ZERO, Vec2::new(100.0, 0.0), 900.0);
        assert!((near.x - 100.0).abs() < 1e-6);
    }
}
