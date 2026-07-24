//! The piracy finisher — the M5 milestone.
//!
//! Combat exists to *take* ships, not just sink them. When an enemy's hull is
//! driven below [`CRIPPLE_THRESHOLD`] it is [`Disabled`](crate::components::Disabled):
//! it stops fighting and drifts. Bring the protagonist within [`BOARD_RANGE`]
//! and raise the [`BoardIntent`] to board it — looting it (counted in
//! [`Plunder`]) and removing it from the fight. You can still just blow it up.

use bevy_ecs::prelude::*;
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{AiController, Disabled, FireOrders, Helm, Hull, Protagonist, Ship};

/// Hull fraction at or below which an enemy is crippled and becomes boardable.
pub const CRIPPLE_THRESHOLD: f32 = 0.25;
/// How close the protagonist must be to board a crippled ship.
pub const BOARD_RANGE: f32 = 95.0;
/// How long the protagonist must hold position within [`BOARD_RANGE`] of a
/// crippled ship to claim it (seconds).
pub const BOARD_DWELL: f32 = 3.0;

/// Running tally of ships boarded (looted) this run — the piracy score.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct Plunder {
    pub ships_boarded: u32,
}

/// The protagonist's intent to board a crippled ship this frame. Kept for the
/// client/AI to raise, though boarding now completes by *dwelling* in range (see
/// [`Boarding`]) rather than on a single keypress.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct BoardIntent {
    pub active: bool,
}

/// Live boarding progress: which crippled ship the protagonist is currently
/// alongside, and for how long. Reset the moment they leave range or the target
/// changes. The client reads this to draw the boarding prompt + progress ring.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct Boarding {
    pub target: Option<Entity>,
    pub progress: f32,
}

/// Bevy system: cripple enemy ships whose hull has fallen low — they stop
/// steering and firing and drift, boardable.
pub fn cripple_system(
    mut commands: Commands,
    // Never cripple the protagonist (it may be AI-piloted via the player-AI toggle).
    mut ships: Query<
        (Entity, &Hull, &mut Helm, &mut FireOrders),
        (
            With<Ship>,
            With<AiController>,
            Without<Disabled>,
            Without<Protagonist>,
        ),
    >,
) {
    for (entity, hull, mut helm, mut orders) in &mut ships {
        if hull.current <= hull.max * CRIPPLE_THRESHOLD {
            commands.entity(entity).insert(Disabled);
            *helm = Helm::default();
            *orders = FireOrders::default();
        }
    }
}

/// Bevy system: board a crippled ship by *holding position* alongside it. While
/// the protagonist stays within [`BOARD_RANGE`] of the nearest crippled ship its
/// [`Boarding`] progress builds; reaching [`BOARD_DWELL`] claims it (looted, +1
/// [`Plunder`]). Drifting out of range, or the target changing, resets progress.
pub fn boarding_system(
    time: Res<Time>,
    mut commands: Commands,
    mut plunder: ResMut<Plunder>,
    mut boarding: ResMut<Boarding>,
    protagonist: Query<&Transform, With<Protagonist>>,
    disabled: Query<(Entity, &Transform), (With<Ship>, With<Disabled>)>,
) {
    let Ok(protagonist) = protagonist.single() else {
        *boarding = Boarding::default();
        return;
    };
    let origin = protagonist.translation.truncate();

    // Nearest crippled ship within board range.
    let mut best: Option<Entity> = None;
    let mut best_dist = BOARD_RANGE * BOARD_RANGE;
    for (entity, transform) in &disabled {
        let dist = transform.translation.truncate().distance_squared(origin);
        if dist <= best_dist {
            best_dist = dist;
            best = Some(entity);
        }
    }

    match best {
        Some(entity) => {
            if boarding.target == Some(entity) {
                boarding.progress += time.delta_secs();
            } else {
                // Just came alongside (or switched prize) — start the dwell.
                boarding.target = Some(entity);
                boarding.progress = 0.0;
            }
            if boarding.progress >= BOARD_DWELL {
                commands.entity(entity).despawn();
                plunder.ships_boarded += 1;
                boarding.target = None;
                boarding.progress = 0.0;
            }
        }
        None => {
            boarding.target = None;
            boarding.progress = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{AiController, Faction, FireOrders, Helm, Hull, Ship};
    use bevy_math::Vec2;

    fn crippled_enemy(world: &mut World, pos: Vec2) -> Entity {
        world
            .spawn((
                Ship,
                Faction::Houses,
                AiController::default(),
                Disabled,
                Hull {
                    current: 10.0,
                    max: 100.0,
                },
                Transform::from_translation(pos.extend(0.0)),
            ))
            .id()
    }

    #[test]
    fn a_low_hull_enemy_becomes_disabled() {
        let mut world = World::new();
        let enemy = world
            .spawn((
                Ship,
                Faction::Houses,
                AiController::default(),
                Hull {
                    current: 20.0,
                    max: 100.0,
                }, // 20% <= 25% threshold
                Helm::default(),
                FireOrders::default(),
            ))
            .id();

        let mut schedule = Schedule::default();
        schedule.add_systems(cripple_system);
        schedule.run(&mut world);

        assert!(
            world.get::<Disabled>(enemy).is_some(),
            "enemy should be crippled"
        );
    }

    /// Insert a `Time` and step a schedule by `secs`, so dwell-based systems can
    /// be driven forward deterministically.
    fn step(world: &mut World, schedule: &mut Schedule, secs: f32) {
        use std::time::Duration;
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(secs));
        schedule.run(world);
    }

    #[test]
    fn boarding_claims_a_ship_after_dwelling_in_range() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Plunder::default());
        world.insert_resource(Boarding::default());
        world.spawn((Protagonist, Transform::from_xyz(0.0, 0.0, 0.0)));
        let enemy = crippled_enemy(&mut world, Vec2::new(40.0, 0.0)); // within BOARD_RANGE

        let mut schedule = Schedule::default();
        schedule.add_systems(boarding_system);

        // One second in range: acquired, but not yet claimed.
        step(&mut world, &mut schedule, 1.0);
        assert!(
            world.get_entity(enemy).is_ok(),
            "not boarded before the dwell"
        );
        assert_eq!(world.resource::<Plunder>().ships_boarded, 0);

        // Past the dwell: the ship is claimed.
        step(&mut world, &mut schedule, BOARD_DWELL);
        assert!(
            world.get_entity(enemy).is_err(),
            "boarded ship should be gone after dwelling"
        );
        assert_eq!(world.resource::<Plunder>().ships_boarded, 1);
    }

    #[test]
    fn leaving_range_resets_boarding_progress() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Plunder::default());
        world.insert_resource(Boarding::default());
        let protagonist = world
            .spawn((Protagonist, Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();
        crippled_enemy(&mut world, Vec2::new(40.0, 0.0)); // in range

        let mut schedule = Schedule::default();
        schedule.add_systems(boarding_system);

        step(&mut world, &mut schedule, 1.0);
        step(&mut world, &mut schedule, 1.0);
        assert!(world.resource::<Boarding>().progress > 0.0, "dwell started");

        // Sail away, out of range: progress must reset.
        *world.get_mut::<Transform>(protagonist).unwrap() = Transform::from_xyz(500.0, 0.0, 0.0);
        step(&mut world, &mut schedule, 1.0);
        assert!(
            world.resource::<Boarding>().target.is_none(),
            "target cleared"
        );
        assert_eq!(world.resource::<Boarding>().progress, 0.0, "progress reset");
        assert_eq!(
            world.resource::<Plunder>().ships_boarded,
            0,
            "nothing claimed"
        );
    }

    #[test]
    fn boarding_misses_a_ship_out_of_range() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Plunder::default());
        world.insert_resource(Boarding::default());
        world.spawn((Protagonist, Transform::from_xyz(0.0, 0.0, 0.0)));
        let enemy = crippled_enemy(&mut world, Vec2::new(500.0, 0.0)); // too far

        let mut schedule = Schedule::default();
        schedule.add_systems(boarding_system);
        step(&mut world, &mut schedule, BOARD_DWELL + 1.0);

        assert!(world.get_entity(enemy).is_ok(), "distant ship survives");
        assert_eq!(world.resource::<Plunder>().ships_boarded, 0);
    }
}
