//! Torpedoes — a reloading magazine of lock-and-release homing missiles.
//!
//! While the pilot holds aim, [`torpedo_aim_system`] locks the hostile nearest
//! the aim cursor and accrues launch count (1 instantly, then one per
//! `lock_interval`, capped by loaded tubes). On release it fires that many
//! [`Torpedo`]s straight up/down off the plane at full speed; their slow
//! rate-limited turn ([`home_velocity`], a 3D axis-angle rotation — no gravity)
//! arcs them over onto the target, and [`torpedo_hit_system`] applies hull damage
//! on a 3D contact. The decidable pieces ([`torpedo_locks`], [`home_velocity`])
//! are unit-tested.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec2, Vec3};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::combat::BRACE_DAMAGE_FACTOR;
use crate::components::{
    Brace, Collider, Faction, Hull, PilotIntent, Ship, Torpedo, TorpedoBay, Ttl,
};
use crate::events::ShipHit;

/// Launch count after holding aim for `elapsed` seconds: one instant lock plus
/// one per `interval`, capped at `cap`.
pub fn torpedo_locks(elapsed: f32, interval: f32, cap: u32) -> u32 {
    let extra = (elapsed / interval.max(1e-3)) as u32;
    (1 + extra).min(cap)
}

/// Turn a 3D velocity toward `desired_dir` by at most `turn_rate * dt`, keeping
/// `speed`. Rotates about the axis `current × desired` — a real angular limit,
/// so a torpedo launched straight up arcs over toward its target instead of
/// snapping. No gravity: only the turn shapes the arc.
pub fn home_velocity(vel: Vec3, desired_dir: Vec3, speed: f32, turn_rate: f32, dt: f32) -> Vec3 {
    let current = vel.normalize_or(desired_dir.normalize_or(Vec3::X));
    let desired = desired_dir.normalize_or(current);
    let dot = current.dot(desired).clamp(-1.0, 1.0);
    let angle = dot.acos();
    if angle < 1e-4 {
        return desired * speed;
    }
    let max = turn_rate * dt;
    let step = angle.min(max);
    // Axis of rotation from current toward desired; fall back to Z if antiparallel.
    let axis = current.cross(desired).try_normalize().unwrap_or(Vec3::Z);
    let new_dir = Quat::from_axis_angle(axis, step) * current;
    new_dir.normalize_or(desired) * speed
}

/// Bevy system: reload torpedo tubes one at a time.
pub fn torpedo_reload_system(time: Res<Time>, mut bays: Query<&mut TorpedoBay>) {
    let dt = time.delta_secs();
    for mut bay in &mut bays {
        let max = bay.tubes_max as f32;
        if bay.loaded < max {
            bay.loaded = (bay.loaded + dt / bay.reload_per_tube).min(max);
        }
    }
}

/// Bevy system: accrue locks while aiming; fire a volley on release. Each bay
/// reads its own [`PilotIntent`], so this drives player and AI ships alike.
pub fn torpedo_aim_system(
    time: Res<Time>,
    mut commands: Commands,
    mut bays: Query<(&Transform, &Faction, &mut TorpedoBay, &PilotIntent)>,
    targets: Query<(Entity, &Transform, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (transform, faction, mut bay, intent) in &mut bays {
        let pos = transform.translation.truncate();

        if intent.torpedo_hold {
            bay.hold_elapsed += dt;
            // Lock the hostile nearest the aim cursor within range.
            let mut best: Option<(Entity, f32)> = None;
            for (entity, t_tf, t_fac) in &targets {
                if !faction.hostile_to(*t_fac) {
                    continue;
                }
                let d = t_tf.translation.truncate().distance(intent.aim_point);
                if d <= bay.lock_radius && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((entity, d));
                }
            }
            bay.target = best.map(|(e, _)| e);
            bay.locks = if bay.target.is_some() {
                let cap = (bay.loaded.floor() as u32).min(bay.tubes_max);
                torpedo_locks(bay.hold_elapsed, bay.lock_interval, cap)
            } else {
                0
            };
        } else {
            // Release edge: fire the locked volley.
            if bay.was_holding && bay.locks > 0 {
                if let Some(target) = bay.target {
                    fire_volley(&mut commands, pos, *faction, &bay, target);
                    bay.loaded = (bay.loaded - bay.locks as f32).max(0.0);
                }
            }
            bay.hold_elapsed = 0.0;
            bay.locks = 0;
            bay.target = None;
        }
        bay.was_holding = intent.torpedo_hold;
    }
}

/// Spawn `bay.locks` torpedoes, launched straight up/down off the plane at full
/// speed. Their slow turn then arcs them over onto the target.
fn fire_volley(
    commands: &mut Commands,
    pos: Vec2,
    faction: Faction,
    bay: &TorpedoBay,
    target: Entity,
) {
    for i in 0..bay.locks {
        let up = if i % 2 == 0 { 1.0 } else { -1.0 };
        let vel = Vec3::new(0.0, 0.0, up * bay.speed);
        commands.spawn((
            Torpedo {
                faction,
                target,
                turn_rate: bay.turn_rate,
                speed: bay.speed,
                damage: bay.damage,
                radius: 12.0,
                vel,
            },
            Transform::from_translation(pos.extend(0.0)),
            Ttl(8.0),
        ));
    }
}

/// Bevy system: steer torpedoes in 3D toward their target and integrate.
pub fn torpedo_homing_system(
    time: Res<Time>,
    targets: Query<&Transform, With<Ship>>,
    mut torps: Query<(&mut Transform, &mut Torpedo), Without<Ship>>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut torp) in &mut torps {
        if let Ok(target_tf) = targets.get(torp.target) {
            let to = target_tf.translation - transform.translation;
            if to.length_squared() > 1e-3 {
                torp.vel = home_velocity(torp.vel, to, torp.speed, torp.turn_rate, dt);
            }
        }
        transform.translation += torp.vel * dt;
    }
}

/// Bevy system: apply hull damage when a torpedo reaches a hostile ship. Uses a
/// 3D distance so a torpedo still arcing high above the plane doesn't detonate.
pub fn torpedo_hit_system(
    mut commands: Commands,
    mut hits: MessageWriter<ShipHit>,
    torps: Query<(Entity, &Transform, &Torpedo), Without<Ship>>,
    mut ships: Query<(&Transform, &Collider, &Faction, &mut Hull, Option<&Brace>), With<Ship>>,
) {
    for (entity, transform, torp) in &torps {
        for (ship_tf, collider, faction, mut hull, brace) in &mut ships {
            if !torp.faction.hostile_to(*faction) {
                continue;
            }
            let reach = torp.radius + collider.radius;
            if transform.translation.distance_squared(ship_tf.translation) <= reach * reach {
                let braced = brace.is_some_and(|b| b.active);
                hull.current -= torp.damage * if braced { BRACE_DAMAGE_FACTOR } else { 1.0 };
                hits.write(ShipHit {
                    position: ship_tf.translation.truncate(),
                    faction: *faction,
                });
                commands.entity(entity).despawn();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_lock_instantly_then_one_per_interval() {
        assert_eq!(torpedo_locks(0.0, 0.5, 10), 1);
        assert_eq!(torpedo_locks(0.5, 0.5, 10), 2);
        assert_eq!(torpedo_locks(1.0, 0.5, 10), 3);
    }

    #[test]
    fn locks_are_capped_by_loaded_tubes() {
        assert_eq!(torpedo_locks(100.0, 0.5, 4), 4);
    }

    #[test]
    fn homing_turns_toward_the_target_but_is_rate_limited() {
        // Launched straight up (+Z), target is +X: after a small step the
        // velocity tilts toward +X but is still mostly +Z (a visible arc).
        let v = home_velocity(Vec3::new(0.0, 0.0, 500.0), Vec3::X, 500.0, 1.0, 0.1);
        assert!((v.length() - 500.0).abs() < 1e-3, "speed preserved");
        assert!(v.x > 0.0, "turned toward the target");
        assert!(
            v.z > 0.0,
            "not turned all the way in one step (still arcing up)"
        );
    }
}
