//! Torpedoes — a reloading magazine of lock-and-release homing missiles.
//!
//! While the pilot holds aim, [`torpedo_lock_system`] *accumulates* locks: the
//! cursor is a brush swept over the hostiles within [`TorpedoBay::range`], taking
//! one target instantly and one more per `lock_interval` (capped by loaded
//! tubes), and a target stays locked once taken even as the cursor moves on. A
//! volley therefore spreads across as many distinct ships as the pilot can sweep
//! ([`next_lock`]), tracked in [`TorpedoLock`]. On release the locked targets
//! hand off into a
//! [`TorpedoLaunchQueue`], independently drained by [`torpedo_launch_system`]
//! one tube at a time ([`TORPEDO_LAUNCH_INTERVAL`] apart) — the two lifecycles
//! are separate components/systems since a volley can still be launching while
//! a fresh aim starts locking. Each launched [`Torpedo`] rises straight up/down
//! off the plane at full speed; their slow rate-limited turn ([`home_velocity`],
//! a 3D axis-angle rotation — no gravity) arcs them onto the target they
//! launched at. Each torpedo keeps that target and self-destructs if it dies;
//! [`torpedo_hit_system`] applies hull damage on a 3D contact. The decidable
//! pieces ([`next_lock`], [`home_velocity`]) are unit-tested.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec3};
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::combat::{braced_damage, spheres_overlap};
use crate::components::{Brace, Collider, Faction, Hull, PilotIntent, Ship, Ttl, ENGAGEMENT_RANGE};
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
    /// How near the aim cursor a hostile must be to be locked — the width of the
    /// brush the pilot sweeps over targets, not the bay's reach (see `range`).
    pub lock_radius: f32,
    /// How far from the ship a hostile may be and still be locked. Matches the
    /// microwarp's jump range ([`ENGAGEMENT_RANGE`]) so the two top-down tools
    /// cover the same ground.
    pub range: f32,
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
            range: ENGAGEMENT_RANGE,
            turn_rate: 3.375, // +50% over the previous 2.25
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

/// The next hostile a sweeping lock should take.
///
/// Prefers the nearest candidate under the cursor that isn't spoken for yet.
/// When everything under the cursor is already locked the answer is normally
/// *nothing* — the pilot is meant to sweep on to the next ship, and stacking
/// tubes on the hull they happen to be resting over would quietly eat the volley.
///
/// The one exception is `more_to_find`: while other unlocked hostiles remain in
/// range, holding still earns nothing; once there are none — a lone survivor, or
/// a formation already fully locked — spare tubes double up rather than go to
/// waste, so a magazine can still be dumped into a single target.
///
/// `near` is ordered nearest-cursor-first; `locked` is the live prefix of
/// [`TorpedoLock::targets`].
pub fn next_lock(near: &[(f32, Entity)], locked: &[Entity], more_to_find: bool) -> Option<Entity> {
    let fresh = near
        .iter()
        .map(|(_, entity)| *entity)
        .find(|entity| !locked.contains(entity));
    match fresh {
        Some(entity) => Some(entity),
        None if more_to_find => None,
        None => near.first().map(|(_, entity)| *entity),
    }
}

/// Drop locks whose ship has died since it was taken, keeping the live entries
/// packed at the front. Returns the new lock count.
fn retain_live_locks(lock: &mut TorpedoLock, alive: impl Fn(Entity) -> bool) -> u32 {
    let mut kept = [None; TORPEDO_TUBES];
    let mut n = 0usize;
    for target in lock.targets.iter().take(lock.locks as usize).flatten() {
        if alive(*target) {
            kept[n] = Some(*target);
            n += 1;
        }
    }
    lock.targets = kept;
    lock.locks = n as u32;
    lock.locks
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
        &Transform,
        &Faction,
        &mut TorpedoBay,
        &mut TorpedoLock,
        &mut TorpedoLaunchQueue,
        &PilotIntent,
    )>,
    targets: Query<(Entity, &Transform, &Faction), With<Ship>>,
) {
    let dt = time.delta_secs();
    for (transform, faction, mut bay, mut lock, mut launch, intent) in &mut bays {
        let ship_pos = transform.translation.truncate();
        if intent.torpedo_hold {
            lock.hold_elapsed += dt;

            // Only *fully* loaded tubes can be locked, and only as many as the
            // launch queue still has room for — so the lock count never promises
            // more torpedoes than will actually leave the ship.
            let free = launch.queue.iter().filter(|slot| slot.is_none()).count() as u32;
            let cap = (bay.loaded.floor() as u32)
                .min(bay.tubes_max)
                .min(TORPEDO_TUBES as u32)
                .min(free);

            // A ship locked a moment ago may have died since; forget it.
            retain_live_locks(&mut lock, |entity| targets.get(entity).is_ok());

            let locked: Vec<Entity> = lock
                .targets
                .iter()
                .take(lock.locks as usize)
                .flatten()
                .copied()
                .collect();

            // Every hostile inside the bay's reach; those also under the cursor
            // are this frame's candidates, nearest to the cursor first.
            let mut near: Vec<(f32, Entity)> = Vec::new();
            let mut more_to_find = false;
            for (entity, t_tf, t_fac) in &targets {
                if !faction.hostile_to(*t_fac) {
                    continue;
                }
                let t_pos = t_tf.translation.truncate();
                if t_pos.distance(ship_pos) > bay.range {
                    continue;
                }
                if !locked.contains(&entity) {
                    more_to_find = true;
                }
                let d = t_pos.distance(intent.aim_point);
                if d <= bay.lock_radius {
                    near.push((d, entity));
                }
            }
            near.sort_by(|a, b| a.0.total_cmp(&b.0));

            // Locks *accumulate* across the whole hold rather than being re-picked
            // from wherever the cursor happens to sit: the pilot sweeps the cursor
            // over a formation and collects a target per tube, and a target stays
            // locked once taken. The first lock is instant, each further one costs
            // `lock_interval` — and that clock only restarts when a lock is
            // actually taken, so hesitating over empty space costs nothing.
            let due = lock.locks == 0 || lock.hold_elapsed >= bay.lock_interval;
            if due && lock.locks < cap {
                if let Some(next) = next_lock(&near, &locked, more_to_find) {
                    let slot = lock.locks as usize;
                    lock.targets[slot] = Some(next);
                    lock.locks += 1;
                    lock.hold_elapsed = 0.0;
                }
            }

            // Tubes can be taken out from under a lock by a volley still
            // launching, so never promise more than the cap allows.
            while lock.locks > cap {
                lock.locks -= 1;
                let slot = lock.locks as usize;
                lock.targets[slot] = None;
            }
        } else {
            // Release edge: hand the locked targets to the staggered launcher and
            // consume the tubes they used. Append into free slots rather than
            // overwriting — a previous volley may still be draining, and clobbering
            // it would silently destroy torpedoes the pilot has already paid for.
            if lock.was_holding && lock.locks > 0 {
                let was_idle = launch.queue.iter().all(|slot| slot.is_none());
                let mut queued = 0u32;
                for target in lock.targets.iter().take(lock.locks as usize).flatten() {
                    match launch.queue.iter_mut().find(|slot| slot.is_none()) {
                        Some(slot) => {
                            *slot = Some(*target);
                            queued += 1;
                        }
                        None => break, // queue full — the cap should have prevented this
                    }
                }
                // Only restart the cadence for a volley arriving into an idle
                // launcher; an in-progress drain keeps its own rhythm.
                if was_idle && queued > 0 {
                    launch.timer = 0.0; // first tube launches next step
                }
                bay.loaded = (bay.loaded - queued as f32).max(0.0);
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
    mut ships: Query<
        (
            Entity,
            &Transform,
            &Collider,
            &Faction,
            &mut Hull,
            Option<&Brace>,
        ),
        With<Ship>,
    >,
) {
    for (entity, transform, torp) in &torps {
        for (ship_entity, ship_tf, collider, faction, mut hull, brace) in &mut ships {
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
                    ship: ship_entity,
                    faction: *faction,
                    damage: torp.damage,
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
    use crate::components::PilotIntent;
    use bevy_ecs::prelude::*;
    use bevy_math::Vec2;

    /// A one-ship world for exercising [`torpedo_lock_system`] frame by frame.
    ///
    /// Locks now accrue one per frame at most, so these tests have to *step* the
    /// schedule rather than advancing the clock once and running it — which is
    /// closer to how the system really runs anyway.
    struct LockHarness {
        world: World,
        schedule: Schedule,
        bay: Entity,
    }

    impl LockHarness {
        fn new(loaded: f32) -> Self {
            let mut world = World::new();
            world.insert_resource(Time::<()>::default());
            let bay = world
                .spawn((
                    Transform::from_xyz(0.0, 0.0, 0.0),
                    Faction::Corsairs,
                    TorpedoBay {
                        loaded,
                        ..Default::default()
                    },
                    TorpedoLock::default(),
                    TorpedoLaunchQueue::default(),
                    PilotIntent::default(),
                ))
                .id();
            let mut schedule = Schedule::default();
            schedule.add_systems(torpedo_lock_system);
            Self {
                world,
                schedule,
                bay,
            }
        }

        fn spawn_hostile(&mut self, pos: Vec2) -> Entity {
            self.world
                .spawn((
                    Ship,
                    Faction::Houses,
                    Transform::from_translation(pos.extend(0.0)),
                ))
                .id()
        }

        /// Hold aim at `cursor` for `frames` steps, each a full `lock_interval`
        /// long so every step is eligible to take a lock.
        fn hold_over(&mut self, cursor: Vec2, frames: u32) {
            let interval = self
                .world
                .get::<TorpedoBay>(self.bay)
                .unwrap()
                .lock_interval;
            for _ in 0..frames {
                {
                    let mut intent = self.world.get_mut::<PilotIntent>(self.bay).unwrap();
                    intent.aim_point = cursor;
                    intent.torpedo_hold = true;
                }
                self.world
                    .resource_mut::<Time<()>>()
                    .advance_by(std::time::Duration::from_secs_f32(interval));
                self.schedule.run(&mut self.world);
            }
        }

        fn locks(&self) -> u32 {
            self.world.get::<TorpedoLock>(self.bay).unwrap().locks
        }

        fn locked_targets(&self) -> Vec<Entity> {
            let lock = self.world.get::<TorpedoLock>(self.bay).unwrap();
            lock.targets
                .iter()
                .take(lock.locks as usize)
                .flatten()
                .copied()
                .collect()
        }
    }

    #[test]
    fn a_sweep_takes_the_nearest_unlocked_hostile() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        let near = [(10.0, a), (30.0, b)];

        assert_eq!(next_lock(&near, &[], true), Some(a), "nearest first");
        assert_eq!(
            next_lock(&near, &[a], true),
            Some(b),
            "skip what is already locked"
        );
    }

    /// With every candidate already locked, spare tubes double up on the nearest
    /// rather than being wasted.
    #[test]
    fn spare_tubes_double_up_once_everything_is_locked() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        let near = [(10.0, a), (30.0, b)];
        assert_eq!(next_lock(&near, &[a, b], false), Some(a));
    }

    #[test]
    fn nothing_under_the_cursor_locks_nothing() {
        assert_eq!(next_lock(&[], &[], true), None);
    }

    /// Locks are limited by *fully* loaded tubes: a partly reloaded magazine can
    /// only throw as many torpedoes as it has whole tubes, and a fraction of a
    /// tube throws nothing at all.
    #[test]
    fn locks_are_limited_by_loaded_tubes() {
        // Hold aim over a target for plenty of intervals; how many lock on?
        let locks_with = |loaded: f32| -> u32 {
            let mut h = LockHarness::new(loaded);
            h.spawn_hostile(Vec2::new(100.0, 0.0));
            h.hold_over(Vec2::new(100.0, 0.0), 10);
            h.locks()
        };

        assert_eq!(locks_with(2.0), 2, "two loaded tubes → two locks");
        assert_eq!(locks_with(2.9), 2, "a part-loaded third tube doesn't count");
        assert_eq!(locks_with(0.8), 0, "no whole tube → nothing to fire");
        assert_eq!(locks_with(6.0), 6, "a full magazine locks every tube");
    }

    /// A hostile beyond the bay's reach can't be locked however long the cursor
    /// sits on it — the bay covers the same ground as the microwarp, no more.
    #[test]
    fn a_target_beyond_the_bays_range_cannot_be_locked() {
        let far = Vec2::new(ENGAGEMENT_RANGE + 50.0, 0.0);
        let mut h = LockHarness::new(6.0);
        h.spawn_hostile(far);
        h.hold_over(far, 10);
        assert_eq!(h.locks(), 0, "out of range should never lock");

        let near = Vec2::new(ENGAGEMENT_RANGE - 50.0, 0.0);
        let mut h = LockHarness::new(6.0);
        h.spawn_hostile(near);
        h.hold_over(near, 10);
        assert!(h.locks() > 0, "just inside range should lock");
    }

    /// The point of accumulating: sweep the cursor across a line of ships and
    /// each one is taken and *kept*, even though the cursor has long since moved
    /// off it. Re-picking from the cursor each frame — the old behaviour — would
    /// leave only the last ship locked.
    #[test]
    fn sweeping_the_cursor_collects_a_target_per_ship() {
        let mut h = LockHarness::new(6.0);
        let ships: Vec<(Entity, Vec2)> = (0..4)
            .map(|i| {
                // Spaced well beyond `lock_radius`, so the cursor can only ever
                // be over one of them at a time.
                let pos = Vec2::new(200.0, i as f32 * 400.0 - 600.0);
                (h.spawn_hostile(pos), pos)
            })
            .collect();

        // One dwell each — every step of `hold_over` is a full lock interval.
        for (_, pos) in &ships {
            h.hold_over(*pos, 1);
        }

        let locked = h.locked_targets();
        assert_eq!(h.locks(), 4, "one lock per ship swept, got {locked:?}");
        for (entity, _) in &ships {
            assert!(
                locked.contains(entity),
                "every ship swept should still be locked: {locked:?}"
            );
        }
    }

    /// Locks survive the cursor wandering off onto empty space.
    #[test]
    fn a_lock_is_kept_once_taken() {
        let target = Vec2::new(200.0, 0.0);
        let mut h = LockHarness::new(6.0);
        let ship = h.spawn_hostile(target);
        // A second hostile far from the cursor, so there is always something
        // left unlocked and the double-up fallback stays out of the way.
        h.spawn_hostile(Vec2::new(0.0, 400.0));

        h.hold_over(target, 1);
        assert_eq!(h.locks(), 1);

        // Drift the cursor onto nothing for a good while.
        h.hold_over(Vec2::new(-400.0, 400.0), 20);

        assert_eq!(h.locks(), 1, "the lock should have been kept");
        assert_eq!(h.locked_targets(), vec![ship]);
    }

    /// A ship destroyed while its lock is held must be forgotten, or the volley
    /// would launch a torpedo at a corpse and immediately self-destruct it.
    #[test]
    fn a_lock_on_a_dead_ship_is_dropped() {
        let target = Vec2::new(200.0, 0.0);
        let mut h = LockHarness::new(6.0);
        let ship = h.spawn_hostile(target);

        h.hold_over(target, 1);
        assert_eq!(h.locked_targets(), vec![ship]);

        h.world.despawn(ship);
        h.hold_over(Vec2::new(-400.0, 400.0), 1);

        assert_eq!(h.locks(), 0, "the dead ship's lock should be gone");
    }

    /// Firing a second volley while the first is still launching must not clobber
    /// the pending tubes — those torpedoes are already paid for.
    #[test]
    fn a_second_volley_does_not_clobber_one_still_launching() {
        use crate::components::PilotIntent;
        use bevy_ecs::prelude::*;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        let ship = world
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                Faction::Corsairs,
                TorpedoBay::default(),
                TorpedoLock::default(),
                // Two tubes from an earlier volley still waiting to launch.
                TorpedoLaunchQueue::default(),
                PilotIntent {
                    aim_point: Vec2::new(100.0, 0.0),
                    torpedo_hold: true,
                    ..Default::default()
                },
            ))
            .id();
        let old = world
            .spawn((Ship, Faction::Houses, Transform::from_xyz(400.0, 0.0, 0.0)))
            .id();
        world.spawn((Ship, Faction::Houses, Transform::from_xyz(100.0, 0.0, 0.0)));
        {
            let mut q = world.get_mut::<TorpedoLaunchQueue>(ship).unwrap();
            q.queue[0] = Some(old);
            q.queue[1] = Some(old);
        }

        let mut schedule = Schedule::default();
        schedule.add_systems(torpedo_lock_system);
        // Lock a fresh volley, then release it.
        world
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        schedule.run(&mut world);
        world.get_mut::<PilotIntent>(ship).unwrap().torpedo_hold = false;
        schedule.run(&mut world);

        let queue = world.get::<TorpedoLaunchQueue>(ship).unwrap();
        let pending: Vec<Entity> = queue.queue.iter().flatten().copied().collect();
        assert_eq!(
            pending.iter().filter(|e| **e == old).count(),
            2,
            "the in-flight volley's tubes must survive: {pending:?}"
        );
        assert!(
            pending.len() > 2,
            "the new volley should append alongside them, got {pending:?}"
        );
    }

    #[test]
    fn a_volley_spreads_across_distinct_targets() {
        // Two hostiles close enough that a single cursor covers both; successive
        // locks must still take one each rather than stacking on the nearer.
        let mut h = LockHarness::new(6.0);
        let a = h.spawn_hostile(Vec2::new(100.0, 0.0));
        let b = h.spawn_hostile(Vec2::new(120.0, 20.0));

        h.hold_over(Vec2::new(100.0, 0.0), 2);

        let locked = h.locked_targets();
        assert_eq!(h.locks(), 2, "should have locked two tubes, got {locked:?}");
        assert!(
            locked.contains(&a) && locked.contains(&b),
            "volley should spread across both ships, got {locked:?}"
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
