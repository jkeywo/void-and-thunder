//! Torpedoes — a reloading magazine of lock-and-release homing missiles.
//!
//! While the pilot holds aim, [`torpedo_aim_system`] locks the hostiles nearest
//! the aim cursor (1 instantly, then one per `lock_interval`, capped by loaded
//! tubes) — a volley spreads across the distinct targets. On release the tubes
//! launch one at a time ([`TORPEDO_LAUNCH_INTERVAL`] apart), each [`Torpedo`]
//! rising straight up/down off the plane at full speed; their slow rate-limited
//! turn ([`home_velocity`], a 3D axis-angle rotation — no gravity) arcs them onto
//! the target they launched at. Each torpedo keeps that target and self-destructs
//! if it dies; [`torpedo_hit_system`] applies hull damage on a 3D contact. The
//! decidable pieces ([`torpedo_locks`], [`home_velocity`]) are unit-tested.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec2, Vec3};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::combat::BRACE_DAMAGE_FACTOR;
use crate::components::{
    Brace, Collider, Faction, Hull, PilotIntent, Ship, Torpedo, TorpedoBay, Ttl, TORPEDO_TUBES,
};
use crate::events::ShipHit;

/// Seconds between successive tube launches once a volley is released.
pub const TORPEDO_LAUNCH_INTERVAL: f32 = 0.5;

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

/// Bevy system: accrue locks while aiming, then launch the volley one tube at a
/// time on release. Each bay reads its own [`PilotIntent`], so this drives player
/// and AI ships alike.
pub fn torpedo_aim_system(
    time: Res<Time>,
    mut commands: Commands,
    mut bays: Query<(&Transform, &Faction, &mut TorpedoBay, &PilotIntent)>,
    targets: Query<(Entity, &Transform, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (transform, faction, mut bay, intent) in &mut bays {
        let pos = transform.translation.truncate();

        // --- Staggered launch: fire one queued tube every launch interval, from
        // the ship's *current* position, so a volley trails out behind it. ---
        if bay.launch_queue.iter().any(|t| t.is_some()) {
            bay.launch_timer -= dt;
            if bay.launch_timer <= 0.0 {
                let (turn_rate, speed, damage, flip, fac) = (
                    bay.turn_rate,
                    bay.speed,
                    bay.damage,
                    bay.launch_flip,
                    *faction,
                );
                if let Some(slot) = bay.launch_queue.iter_mut().find(|t| t.is_some()) {
                    let target = slot.take().unwrap();
                    let up = if flip { 1.0 } else { -1.0 };
                    commands.spawn((
                        Torpedo {
                            faction: fac,
                            target,
                            turn_rate,
                            speed,
                            damage,
                            radius: 12.0,
                            vel: Vec3::new(0.0, 0.0, up * speed),
                        },
                        Transform::from_translation(pos.extend(0.0)),
                        Ttl(8.0),
                    ));
                }
                bay.launch_flip = !bay.launch_flip;
                bay.launch_timer = TORPEDO_LAUNCH_INTERVAL;
            }
        }

        if intent.torpedo_hold {
            bay.hold_elapsed += dt;
            // Hostiles within lock range of the cursor, nearest first.
            let mut near: Vec<(f32, Entity)> = Vec::new();
            for (entity, t_tf, t_fac) in &targets {
                if !faction.hostile_to(*t_fac) {
                    continue;
                }
                let d = t_tf.translation.truncate().distance(intent.aim_point);
                if d <= bay.lock_radius {
                    near.push((d, entity));
                }
            }
            near.sort_by(|a, b| a.0.total_cmp(&b.0));

            let cap = (bay.loaded.floor() as u32)
                .min(bay.tubes_max)
                .min(TORPEDO_TUBES as u32);
            bay.locks = if near.is_empty() {
                0
            } else {
                torpedo_locks(bay.hold_elapsed, bay.lock_interval, cap)
            };
            // Spread the volley across the nearest distinct hostiles; extra tubes
            // wrap round to double up on a lone target.
            bay.targets = [None; TORPEDO_TUBES];
            for i in 0..bay.locks as usize {
                bay.targets[i] = Some(near[i % near.len()].1);
            }
        } else {
            // Release edge: hand the locked targets to the staggered launcher.
            if bay.was_holding && bay.locks > 0 {
                bay.launch_queue = bay.targets;
                bay.launch_timer = 0.0; // first tube launches next step
                bay.loaded = (bay.loaded - bay.locks as f32).max(0.0);
            }
            bay.hold_elapsed = 0.0;
            bay.locks = 0;
            bay.targets = [None; TORPEDO_TUBES];
        }
        bay.was_holding = intent.torpedo_hold;
    }
}

/// Bevy system: steer torpedoes in 3D toward their target and integrate. A
/// torpedo keeps the target it launched at; if that ship is gone (destroyed or
/// boarded) it self-destructs rather than flying on blindly.
pub fn torpedo_homing_system(
    time: Res<Time>,
    mut commands: Commands,
    targets: Query<&Transform, With<Ship>>,
    mut torps: Query<(Entity, &mut Transform, &mut Torpedo), Without<Ship>>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut torp) in &mut torps {
        let Ok(target_tf) = targets.get(torp.target) else {
            commands.entity(entity).despawn();
            continue;
        };
        let to = target_tf.translation - transform.translation;
        if to.length_squared() > 1e-3 {
            torp.vel = home_velocity(torp.vel, to, torp.speed, torp.turn_rate, dt);
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
    fn a_volley_spreads_across_distinct_targets() {
        use crate::components::PilotIntent;
        use bevy_ecs::prelude::*;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());

        // A Corsair bay aiming at (100, 0).
        let bay = world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                Faction::Corsairs,
                TorpedoBay::default(),
                PilotIntent {
                    aim_point: Vec2::new(100.0, 0.0),
                    torpedo_hold: true,
                    ..Default::default()
                },
            ))
            .id();
        // Two House ships within lock range of the cursor.
        let a = world
            .spawn((Ship, Faction::Houses, Transform::from_xyz(100.0, 0.0, 0.0)))
            .id();
        let b = world
            .spawn((Ship, Faction::Houses, Transform::from_xyz(120.0, 20.0, 0.0)))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(torpedo_aim_system);
        // 0.6s of holding → one instant lock plus one more (>= lock_interval).
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        schedule.run(&mut world);

        let bay = world.get::<TorpedoBay>(bay).unwrap();
        assert_eq!(bay.locks, 2, "should have locked two tubes");
        let locked: Vec<Entity> = bay.targets.iter().flatten().copied().collect();
        assert!(
            locked.contains(&a) && locked.contains(&b),
            "volley should spread across both ships"
        );
    }

    #[test]
    fn a_torpedo_self_destructs_when_its_target_is_gone() {
        use bevy_ecs::prelude::*;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        // A target id that no longer exists.
        let ghost = world.spawn_empty().id();
        world.despawn(ghost);

        let torp = world
            .spawn((
                Torpedo {
                    faction: Faction::Corsairs,
                    target: ghost,
                    turn_rate: 2.25,
                    speed: 260.0,
                    damage: 22.0,
                    radius: 12.0,
                    vel: Vec3::new(0.0, 0.0, 260.0),
                },
                Transform::default(),
                Ttl(8.0),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(torpedo_homing_system);
        schedule.run(&mut world);

        assert!(
            world.get_entity(torp).is_err(),
            "torpedo should self-destruct when its target is gone"
        );
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
