//! The piracy finisher — the M5 milestone.
//!
//! Combat exists to *take* ships, not just sink them. When an enemy's hull is
//! driven below [`CRIPPLE_THRESHOLD`] it is [`Disabled`](crate::components::Disabled):
//! it stops fighting and drifts. Bring the protagonist within [`BOARD_RANGE`]
//! and raise the [`BoardIntent`] to board it — looting it (counted in
//! [`Plunder`]) and removing it from the fight. You can still just blow it up.

use bevy_ecs::prelude::*;
use bevy_transform::components::Transform;

use crate::components::{AiController, Disabled, FireOrders, Helm, Hull, Protagonist, Ship};

/// Hull fraction at or below which an enemy is crippled and becomes boardable.
pub const CRIPPLE_THRESHOLD: f32 = 0.25;
/// How close the protagonist must be to board a crippled ship.
pub const BOARD_RANGE: f32 = 95.0;

/// Running tally of ships boarded (looted) this run — the piracy score.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct Plunder {
    pub ships_boarded: u32,
}

/// The protagonist's intent to board a crippled ship this frame. The client
/// raises it on a keypress; [`boarding_system`] consumes it.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct BoardIntent {
    pub active: bool,
}

/// Bevy system: cripple enemy ships whose hull has fallen low — they stop
/// steering and firing and drift, boardable.
pub fn cripple_system(
    mut commands: Commands,
    mut ships: Query<
        (Entity, &Hull, &mut Helm, &mut FireOrders),
        (With<Ship>, With<AiController>, Without<Disabled>),
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

/// Bevy system: board the nearest crippled ship in range when the protagonist
/// raises [`BoardIntent`].
pub fn boarding_system(
    mut commands: Commands,
    mut intent: ResMut<BoardIntent>,
    mut plunder: ResMut<Plunder>,
    protagonist: Query<&Transform, With<Protagonist>>,
    disabled: Query<(Entity, &Transform), (With<Ship>, With<Disabled>)>,
) {
    if !intent.active {
        return;
    }
    intent.active = false; // one board attempt per raised intent

    let Ok(protagonist) = protagonist.single() else {
        return;
    };
    let origin = protagonist.translation.truncate();

    let mut best: Option<Entity> = None;
    let mut best_dist = BOARD_RANGE * BOARD_RANGE;
    for (entity, transform) in &disabled {
        let dist = transform.translation.truncate().distance_squared(origin);
        if dist <= best_dist {
            best_dist = dist;
            best = Some(entity);
        }
    }

    if let Some(entity) = best {
        commands.entity(entity).despawn();
        plunder.ships_boarded += 1;
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

    #[test]
    fn boarding_takes_a_crippled_ship_in_range() {
        let mut world = World::new();
        world.insert_resource(Plunder::default());
        world.insert_resource(BoardIntent { active: true });
        world.spawn((Protagonist, Transform::from_xyz(0.0, 0.0, 0.0)));
        let enemy = crippled_enemy(&mut world, Vec2::new(40.0, 0.0)); // within BOARD_RANGE

        let mut schedule = Schedule::default();
        schedule.add_systems(boarding_system);
        schedule.run(&mut world);

        assert!(
            world.get_entity(enemy).is_err(),
            "boarded ship should be gone"
        );
        assert_eq!(world.resource::<Plunder>().ships_boarded, 1);
        assert!(
            !world.resource::<BoardIntent>().active,
            "intent is consumed"
        );
    }

    #[test]
    fn boarding_misses_a_ship_out_of_range() {
        let mut world = World::new();
        world.insert_resource(Plunder::default());
        world.insert_resource(BoardIntent { active: true });
        world.spawn((Protagonist, Transform::from_xyz(0.0, 0.0, 0.0)));
        let enemy = crippled_enemy(&mut world, Vec2::new(500.0, 0.0)); // too far

        let mut schedule = Schedule::default();
        schedule.add_systems(boarding_system);
        schedule.run(&mut world);

        assert!(world.get_entity(enemy).is_ok(), "distant ship survives");
        assert_eq!(world.resource::<Plunder>().ships_boarded, 0);
    }
}
