//! Torpedoes — a reloading magazine of lock-and-release homing missiles.
//!
//! While the pilot holds aim, [`torpedo_lock_system`] locks the hostiles nearest
//! the aim cursor (1 instantly, then one per `lock_interval`, capped by loaded
//! tubes) — a volley spreads across the distinct targets, tracked in
//! [`TorpedoLock`]. On release the locked targets hand off into a
//! [`TorpedoLaunchQueue`], independently drained by [`torpedo_launch_system`]
//! one tube at a time ([`TORPEDO_LAUNCH_INTERVAL`] apart) — the two lifecycles
//! are separate components/systems since a volley can still be launching while
//! a fresh aim starts locking. Each launched [`Torpedo`] rises straight up/down
//! off the plane at full speed; their slow rate-limited turn ([`home_velocity`],
//! a 3D axis-angle rotation — no gravity) arcs them onto the target they
//! launched at. Each torpedo keeps that target and self-destructs if it dies;
//! [`torpedo_hit_system`] applies hull damage on a 3D contact. The decidable
//! pieces ([`torpedo_locks`], [`home_velocity`]) are unit-tested.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec3};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::combat::{braced_damage, spheres_overlap};
use crate::components::{Brace, Collider, Faction, Hull, PilotIntent, Ship, Ttl};
use crate::events::ShipHit;

/// Seconds between successive tube launches once a volley is released.
pub const TORPEDO_LAUNCH_INTERVAL: f32 = 0.5;

/// Maximum torpedo tubes a bay can hold. Sizes the fixed lock/launch arrays so
/// [`TorpedoBay`] can stay `Copy`.
pub const TORPEDO_TUBES: usize = 6;

/// A magazine of homing torpedoes: static config plus reload state, unchanged
/// across a ship's life. Tubes reload one at a time. The two transient
/// lifecycles that ride alongside this — accruing locks while aiming
/// ([`TorpedoLock`]) and draining a staggered launch ([`TorpedoLaunchQueue`]) —
/// are separate components, since they don't share a lifetime with each other
/// or with this one (a volley can be mid-launch while a fresh aim starts).
#[derive(Component, Clone, Copy, Debug)]
pub struct TorpedoBay {
    /// Tubes available (magazine size).
    pub tubes_max: u32,
    /// Tubes currently loaded (fractional while a tube reloads).
    pub loaded: f32,
    /// Seconds to reload one tube.
    pub reload_per_tube: f32,
    /// Seconds between additional locks while holding aim.
    pub lock_interval: f32,
    /// How near the aim cursor a hostile must be to be locked.
    pub lock_radius: f32,
    /// Homing turn rate (radians/s) — kept moderate so the launch arc is visible.
    pub turn_rate: f32,
    /// Cruise speed (units/s); torpedoes launch and fly at this the whole way.
    pub speed: f32,
    /// Hull damage per torpedo.
    pub damage: f32,
}

impl Default for TorpedoBay {
    fn default() -> Self {
        Self {
            tubes_max: 6,
            loaded: 6.0,
            reload_per_tube: 1.5,
            lock_interval: 0.5,
            lock_radius: 112.5, // 75% of the original 150
            turn_rate: 2.25,    // +50% over the original 1.5
            speed: 260.0,
            damage: 22.0,
        }
    }
}

/// Transient lock-while-aiming state for a [`TorpedoBay`], reset every time the
/// pilot releases aim.
#[derive(Component, Clone, Copy, Debug)]
pub struct TorpedoLock {
    pub hold_elapsed: f32,
    pub was_holding: bool,
    /// Locks accrued while aiming (also the number of live entries in `targets`).
    pub locks: u32,
    /// The hostiles locked while aiming — a volley spreads across these.
    pub targets: [Option<Entity>; TORPEDO_TUBES],
}

impl Default for TorpedoLock {
    fn default() -> Self {
        Self {
            hold_elapsed: 0.0,
            was_holding: false,
            locks: 0,
            targets: [None; TORPEDO_TUBES],
        }
    }
}

/// Transient staggered-launch state for a [`TorpedoBay`], populated on
/// release and drained one tube per `TORPEDO_LAUNCH_INTERVAL`.
#[derive(Component, Clone, Copy, Debug)]
pub struct TorpedoLaunchQueue {
    /// Targets still waiting to launch; one tube fires each launch interval.
    pub queue: [Option<Entity>; TORPEDO_TUBES],
    /// Countdown to the next tube launch.
    pub timer: f32,
    /// Alternates each launch so consecutive tubes fire up/down off the plane.
    pub flip: bool,
}

impl Default for TorpedoLaunchQueue {
    fn default() -> Self {
        Self {
            queue: [None; TORPEDO_TUBES],
            timer: 0.0,
            flip: false,
        }
    }
}

/// A homing torpedo in flight, chasing `target`. It carries its own 3D velocity
/// (it launches straight up/down off the plane and arcs over), so it does not
/// use the flat [`Velocity`](crate::components::Velocity) component.
#[derive(Component, Clone, Copy, Debug)]
pub struct Torpedo {
    pub faction: Faction,
    pub target: Entity,
    pub turn_rate: f32,
    pub speed: f32,
    pub damage: f32,
    pub radius: f32,
    /// Full 3D velocity.
    pub vel: Vec3,
}

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

/// Bevy system: drain each ship's staggered launch queue, firing one tube
/// every [`TORPEDO_LAUNCH_INTERVAL`] from the ship's *current* position, so a
/// volley trails out behind it. Independent of the aim/lock lifecycle — a
/// volley can still be launching while a fresh aim starts accruing locks.
pub fn torpedo_launch_system(
    time: Res<Time>,
    mut commands: Commands,
    mut bays: Query<(&Transform, &Faction, &TorpedoBay, &mut TorpedoLaunchQueue)>,
) {
    let dt = time.delta_secs();
    for (transform, faction, bay, mut launch) in &mut bays {
        if !launch.queue.iter().any(|t| t.is_some()) {
            continue;
        }
        let pos = transform.translation.truncate();
        launch.timer -= dt;
        if launch.timer <= 0.0 {
            let (turn_rate, speed, damage, flip, fac) =
                (bay.turn_rate, bay.speed, bay.damage, launch.flip, *faction);
            if let Some(slot) = launch.queue.iter_mut().find(|t| t.is_some()) {
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
            launch.flip = !launch.flip;
            launch.timer = TORPEDO_LAUNCH_INTERVAL;
        }
    }
}

/// Bevy system: accrue locks while aiming, then on release hand the locked
/// targets to the staggered launch queue. Each ship reads its own
/// [`PilotIntent`], so this drives player and AI ships alike. Lock-accrual and
/// the release-edge hand-off are one same-entity, same-frame data dependency
/// (both touch [`TorpedoLock`] and [`TorpedoLaunchQueue`] together), unlike the
/// independent drain in [`torpedo_launch_system`].
pub fn torpedo_lock_system(
    time: Res<Time>,
    mut bays: Query<(
        &Faction,
        &mut TorpedoBay,
        &mut TorpedoLock,
        &mut TorpedoLaunchQueue,
        &PilotIntent,
    )>,
    targets: Query<(Entity, &Transform, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (faction, mut bay, mut lock, mut launch, intent) in &mut bays {
        if intent.torpedo_hold {
            lock.hold_elapsed += dt;
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
            lock.locks = if near.is_empty() {
                0
            } else {
                torpedo_locks(lock.hold_elapsed, bay.lock_interval, cap)
            };
            // Spread the volley across the nearest distinct hostiles; extra tubes
            // wrap round to double up on a lone target.
            lock.targets = [None; TORPEDO_TUBES];
            for i in 0..lock.locks as usize {
                lock.targets[i] = Some(near[i % near.len()].1);
            }
        } else {
            // Release edge: hand the locked targets to the staggered launcher
            // and consume the loaded tubes they used.
            if lock.was_holding && lock.locks > 0 {
                launch.queue = lock.targets;
                launch.timer = 0.0; // first tube launches next step
                bay.loaded = (bay.loaded - lock.locks as f32).max(0.0);
            }
            lock.hold_elapsed = 0.0;
            lock.locks = 0;
            lock.targets = [None; TORPEDO_TUBES];
        }
        lock.was_holding = intent.torpedo_hold;
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
            if spheres_overlap(
                transform.translation,
                torp.radius,
                ship_tf.translation,
                collider.radius,
            ) {
                hull.current -= braced_damage(torp.damage, brace.is_some_and(|b| b.active));
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
    use bevy_math::Vec2;

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
                TorpedoLock::default(),
                TorpedoLaunchQueue::default(),
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
        schedule.add_systems(torpedo_lock_system);
        // 0.6s of holding → one instant lock plus one more (>= lock_interval).
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        schedule.run(&mut world);

        let lock = world.get::<TorpedoLock>(bay).unwrap();
        assert_eq!(lock.locks, 2, "should have locked two tubes");
        let locked: Vec<Entity> = lock.targets.iter().flatten().copied().collect();
        assert!(
            locked.contains(&a) && locked.contains(&b),
            "volley should spread across both ships"
        );
    }

    #[test]
    fn queued_torpedoes_launch_one_per_interval() {
        use bevy_ecs::prelude::*;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());

        // A target for the queued torpedo to home on (never actually checked
        // here — this test only isolates the launch-queue drain).
        let target = world
            .spawn((Ship, Faction::Houses, Transform::from_xyz(500.0, 0.0, 0.0)))
            .id();

        // A bay with one tube already queued to launch, no lock state at all —
        // this is the isolation `torpedo_aim_system`'s single-function version
        // couldn't test on its own.
        let ship = world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                Faction::Corsairs,
                TorpedoBay::default(),
                TorpedoLaunchQueue {
                    queue: {
                        let mut q = [None; TORPEDO_TUBES];
                        q[0] = Some(target);
                        q
                    },
                    timer: 0.0,
                    flip: false,
                },
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(torpedo_launch_system);
        schedule.run(&mut world);

        let launch = world.get::<TorpedoLaunchQueue>(ship).unwrap();
        assert!(
            launch.queue.iter().all(|t| t.is_none()),
            "the queued tube should have launched"
        );
        assert_eq!(
            world
                .query::<&Torpedo>()
                .iter(&world)
                .filter(|t| t.target == target)
                .count(),
            1,
            "exactly one torpedo should have spawned"
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
