//! The EMP emitter — a frontal auto-tracking weapon that fills a target's EMP
//! bar (slowing it), rather than damaging its hull.
//!
//! While the pilot holds EMP, [`emp_system`] picks the hostile ship nearest the
//! aim cursor within the emitter's range and arc, swivels the emitter toward a
//! simple lead of that target (capped by `swivel_rate`, clamped to the arc), and
//! fires bolts on cooldown. [`emp_bolt_system`] flies the bolts and applies EMP
//! on contact. The aiming maths is the pure [`lead_angle`]/[`swivel_toward`] so
//! it is unit-testable.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::combat::circles_overlap;
use crate::components::{Collider, EmpDefense, Faction, Heading, PilotIntent, Ship, Ttl, Velocity};
use crate::events::EmpImpact;
use crate::util::wrap_angle as wrap;

/// A frontal EMP emitter. While held it swivels within `arc` toward a lead of
/// the target and fires [`EmpBolt`]s on `cooldown`. Player-only for now.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct EmpWeapon {
    /// Full width of the firing arc (radians); the emitter aims within ±arc/2 of the bow.
    pub arc: f32,
    /// Maximum swivel speed of the emitter (radians/s).
    pub swivel_rate: f32,
    /// Current emitter angle relative to the bow (`-arc/2..=arc/2`).
    pub aim: f32,
    pub cooldown: f32,
    pub timer: f32,
    /// Speed of a fired bolt (units/s).
    pub bolt_speed: f32,
    /// EMP dealt per bolt, as a fraction of the *target's* resist (0.25 = 25%).
    pub bolt_damage_frac: f32,
    /// Maximum engagement range.
    pub range: f32,
}

impl Default for EmpWeapon {
    fn default() -> Self {
        use std::f32::consts::FRAC_PI_2;
        Self {
            arc: FRAC_PI_2,                     // 90°
            swivel_rate: 20.0_f32.to_radians(), // ~1s per 20°
            aim: 0.0,
            cooldown: 0.4,
            timer: 0.0,
            bolt_speed: 360.0,
            bolt_damage_frac: 0.25,
            range: 620.0,
        }
    }
}

/// An EMP bolt in flight. On hitting a hostile ship it adds
/// `damage_frac * target.resist` to that ship's [`EmpDefense`].
#[derive(Component, Clone, Copy, Debug)]
pub struct EmpBolt {
    pub faction: Faction,
    pub radius: f32,
    pub damage_frac: f32,
}

/// Predicted intercept point for a bolt of speed `bolt_speed` fired now at a
/// target moving at `target_vel` (one-step lead).
pub fn lead_point(shooter: Vec2, target_pos: Vec2, target_vel: Vec2, bolt_speed: f32) -> Vec2 {
    let t = shooter.distance(target_pos) / bolt_speed.max(1.0);
    target_pos + target_vel * t
}

/// Desired emitter angle (relative to the bow), clamped to `±arc/2`.
pub fn lead_angle(
    shooter: Vec2,
    heading: f32,
    target_pos: Vec2,
    target_vel: Vec2,
    bolt_speed: f32,
    arc: f32,
) -> f32 {
    let point = lead_point(shooter, target_pos, target_vel, bolt_speed);
    let rel = wrap((point - shooter).to_angle() - heading);
    rel.clamp(-arc * 0.5, arc * 0.5)
}

/// Move `current` toward `desired` by at most `max_delta`.
pub fn swivel_toward(current: f32, desired: f32, max_delta: f32) -> f32 {
    let diff = desired - current;
    if diff.abs() <= max_delta {
        desired
    } else {
        current + max_delta * diff.signum()
    }
}

/// Bevy system: swivel each EMP emitter toward its target and fire on cooldown.
/// Each ship reads its own [`PilotIntent`], so player and AI ships use the same
/// weapon system — only the controller writing the intent differs.
pub fn emp_system(
    time: Res<Time>,
    mut commands: Commands,
    mut shooters: Query<(&Transform, &Heading, &Faction, &mut EmpWeapon, &PilotIntent)>,
    targets: Query<(&Transform, &Velocity, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (transform, heading, faction, mut emp, intent) in &mut shooters {
        emp.timer = (emp.timer - dt).max(0.0);
        if !intent.emp_fire {
            continue;
        }
        let pos = transform.translation.truncate();

        // Nearest hostile to the aim cursor, within range and arc.
        let mut best: Option<(Vec2, Vec2)> = None;
        let mut best_dist = f32::MAX;
        for (t_tf, t_vel, t_fac) in &targets {
            if !faction.hostile_to(*t_fac) {
                continue;
            }
            let tp = t_tf.translation.truncate();
            if pos.distance(tp) > emp.range {
                continue;
            }
            if wrap((tp - pos).to_angle() - heading.0).abs() > emp.arc * 0.5 {
                continue;
            }
            let d = tp.distance(intent.aim_point);
            if d < best_dist {
                best_dist = d;
                best = Some((tp, t_vel.0));
            }
        }

        // Swivel toward the target's lead (or hold if none).
        if let Some((tp, tv)) = best {
            let desired = lead_angle(pos, heading.0, tp, tv, emp.bolt_speed, emp.arc);
            emp.aim = swivel_toward(emp.aim, desired, emp.swivel_rate * dt);

            if emp.timer <= 0.0 {
                let dir = Vec2::from_angle(heading.0 + emp.aim);
                let muzzle = pos + dir * 26.0;
                commands.spawn((
                    EmpBolt {
                        faction: *faction,
                        radius: 6.0,
                        damage_frac: emp.bolt_damage_frac,
                    },
                    Velocity(dir * emp.bolt_speed),
                    Transform::from_translation(muzzle.extend(0.0)),
                    Ttl(emp.range / emp.bolt_speed + 0.2),
                ));
                emp.timer = emp.cooldown;
            }
        }
    }
}

/// Bevy system: fly EMP bolts and apply EMP on contact with a hostile hull.
pub fn emp_bolt_system(
    time: Res<Time>,
    mut commands: Commands,
    mut impacts: MessageWriter<EmpImpact>,
    mut bolts: Query<(Entity, &mut Transform, &Velocity, &mut Ttl, &EmpBolt), Without<Ship>>,
    mut ships: Query<(&Transform, &Collider, &Faction, &mut EmpDefense), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, velocity, mut ttl, bolt) in &mut bolts {
        transform.translation += (velocity.0 * dt).extend(0.0);
        ttl.0 -= dt;
        let pos = transform.translation.truncate();

        let mut hit = false;
        for (ship_tf, collider, faction, mut emp) in &mut ships {
            if !bolt.faction.hostile_to(*faction) {
                continue;
            }
            if circles_overlap(
                pos,
                bolt.radius,
                ship_tf.translation.truncate(),
                collider.radius,
            ) {
                emp.damage = (emp.damage + bolt.damage_frac * emp.resist).min(emp.resist);
                impacts.write(EmpImpact { position: pos });
                hit = true;
                break;
            }
        }
        if hit || ttl.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn lead_aims_ahead_of_a_crossing_target() {
        // Target at (100,0) moving +Y; the lead point should be above it.
        let point = lead_point(
            Vec2::ZERO,
            Vec2::new(100.0, 0.0),
            Vec2::new(0.0, 50.0),
            500.0,
        );
        assert!(
            point.y > 0.0,
            "lead should be ahead of the target, got {point:?}"
        );
    }

    #[test]
    fn emitter_angle_is_clamped_to_the_arc() {
        // Target straight behind (angle PI) but arc is 90° => clamp to +/-45°.
        let a = lead_angle(
            Vec2::ZERO,
            0.0,
            Vec2::new(-100.0, 1.0),
            Vec2::ZERO,
            500.0,
            FRAC_PI_2,
        );
        assert!(
            a.abs() <= FRAC_PI_2 * 0.5 + 1e-4,
            "angle {a} exceeds the arc"
        );
    }

    #[test]
    fn swivel_is_rate_limited_then_snaps() {
        // Far from desired: moves exactly max_delta.
        let a = swivel_toward(0.0, 1.0, 0.1);
        assert!((a - 0.1).abs() < 1e-6);
        // Within max_delta: snaps to desired.
        let b = swivel_toward(0.95, 1.0, 0.1);
        assert!((b - 1.0).abs() < 1e-6);
    }
}
