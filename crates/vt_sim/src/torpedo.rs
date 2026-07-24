//! Torpedoes — a reloading magazine of lock-and-release homing missiles.
//!
//! While the pilot holds aim, [`torpedo_aim_system`] locks the hostile nearest
//! the aim cursor and accrues launch count (1 instantly, then one per
//! `lock_interval`, capped by loaded tubes). On release it fires that many
//! [`Torpedo`]s from alternating top/bottom of the ship; [`torpedo_homing_system`]
//! steers them toward the target while their launch arc settles back to the
//! plane, and [`torpedo_hit_system`] applies hull damage on contact. The
//! decidable pieces ([`torpedo_locks`], [`home_velocity`]) are unit-tested.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::combat::{circles_overlap, BRACE_DAMAGE_FACTOR};
use crate::components::{
    Brace, Collider, Faction, Hull, PilotIntent, Ship, Torpedo, TorpedoBay, Ttl, Velocity,
};
use crate::events::ShipHit;

/// Launch count after holding aim for `elapsed` seconds: one instant lock plus
/// one per `interval`, capped at `cap`.
pub fn torpedo_locks(elapsed: f32, interval: f32, cap: u32) -> u32 {
    let extra = (elapsed / interval.max(1e-3)) as u32;
    (1 + extra).min(cap)
}

/// Turn `vel` toward `desired_dir` by at most `turn_rate * dt`, keeping `speed`.
pub fn home_velocity(vel: Vec2, desired_dir: Vec2, speed: f32, turn_rate: f32, dt: f32) -> Vec2 {
    let current = vel.normalize_or(desired_dir.normalize_or(Vec2::X));
    let desired = desired_dir.normalize_or(current);
    let max = turn_rate * dt;
    // Signed angle from current to desired, clamped to the max turn.
    let cross = current.x * desired.y - current.y * desired.x;
    let dot = current.dot(desired).clamp(-1.0, 1.0);
    let angle = dot.acos().copysign(cross).clamp(-max, max);
    Vec2::from_angle(current.to_angle() + angle) * speed
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

/// Bevy system: accrue locks while aiming; fire a volley on release.
pub fn torpedo_aim_system(
    time: Res<Time>,
    intent: Res<PilotIntent>,
    mut commands: Commands,
    mut bays: Query<(&Transform, &Faction, &mut TorpedoBay)>,
    targets: Query<(Entity, &Transform, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (transform, faction, mut bay) in &mut bays {
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
                    // Launch toward the target's current position.
                    let target_pos = targets
                        .get(target)
                        .map(|(_, tf, _)| tf.translation.truncate())
                        .unwrap_or(intent.aim_point);
                    fire_volley(&mut commands, pos, target_pos, *faction, &bay, target);
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

/// Spawn `bay.locks` torpedoes from alternating top/bottom of the ship, launched
/// toward the target with a slight fan.
fn fire_volley(
    commands: &mut Commands,
    pos: Vec2,
    target_pos: Vec2,
    faction: Faction,
    bay: &TorpedoBay,
    target: Entity,
) {
    let toward = (target_pos - pos).normalize_or(Vec2::X);
    let base_angle = toward.to_angle();
    for i in 0..bay.locks {
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let z = side * bay.arc_height;
        // Fan the launch headings slightly so a volley spreads then converges.
        let spread = (i as f32 - (bay.locks as f32 - 1.0) * 0.5) * 0.12;
        let dir = Vec2::from_angle(base_angle + spread);
        commands.spawn((
            Torpedo {
                faction,
                target,
                turn_rate: bay.turn_rate,
                speed: bay.speed,
                damage: bay.damage,
                radius: 9.0,
            },
            Velocity(dir * bay.speed),
            Transform::from_translation(pos.extend(z)),
            Ttl(6.0),
        ));
    }
}

/// Bevy system: home torpedoes toward their target and settle their arc to the plane.
pub fn torpedo_homing_system(
    time: Res<Time>,
    targets: Query<&Transform, With<Ship>>,
    mut torps: Query<(&mut Transform, &mut Velocity, &Torpedo), Without<Ship>>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut velocity, torp) in &mut torps {
        let pos = transform.translation.truncate();
        if let Ok(target_tf) = targets.get(torp.target) {
            let to = target_tf.translation.truncate() - pos;
            if to.length_squared() > 1e-3 {
                velocity.0 = home_velocity(velocity.0, to, torp.speed, torp.turn_rate, dt);
            }
        }
        transform.translation += (velocity.0 * dt).extend(0.0);
        // Ease the launch arc back down to the plane.
        transform.translation.z += (0.0 - transform.translation.z) * (dt * 2.5).min(1.0);
    }
}

/// Bevy system: apply hull damage when a torpedo reaches a hostile ship.
pub fn torpedo_hit_system(
    mut commands: Commands,
    mut hits: MessageWriter<ShipHit>,
    torps: Query<(Entity, &Transform, &Torpedo), Without<Ship>>,
    mut ships: Query<(&Transform, &Collider, &Faction, &mut Hull, Option<&Brace>), With<Ship>>,
) {
    for (entity, transform, torp) in &torps {
        let pos = transform.translation.truncate();
        for (ship_tf, collider, faction, mut hull, brace) in &mut ships {
            if !torp.faction.hostile_to(*faction) {
                continue;
            }
            let ship_pos = ship_tf.translation.truncate();
            if circles_overlap(pos, torp.radius, ship_pos, collider.radius) {
                let braced = brace.is_some_and(|b| b.active);
                hull.current -= torp.damage * if braced { BRACE_DAMAGE_FACTOR } else { 1.0 };
                hits.write(ShipHit {
                    position: ship_pos,
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
        // Flying +X, target is +Y: after a small step, velocity should have
        // rotated toward +Y but not all the way.
        let v = home_velocity(Vec2::new(500.0, 0.0), Vec2::new(0.0, 1.0), 500.0, 1.0, 0.1);
        assert!((v.length() - 500.0).abs() < 1e-3, "speed preserved");
        assert!(v.y > 0.0, "turned toward the target");
        assert!(v.x > 0.0, "not turned all the way in one step");
    }
}
