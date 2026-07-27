//! Point defence: a battery-fed bubble that swats incoming munitions.
//!
//! The defensive third of the battery slot, against the boost drive's mobility
//! and the disruptor's offence. While the pilot holds it and the ship's
//! [`Battery`](crate::drive::Battery) can pay, the emitter picks the nearest
//! hostile munition inside `radius` and destroys it, at most one every `1/rate`
//! seconds.
//!
//! It answers *everything* thrown at the ship — cannon shot, EMP bolts and
//! torpedoes alike — because a screen that only stops one of the three is a
//! screen the player cannot reason about mid-fight. What it costs is the pool
//! that would otherwise have been an escape or a disable, which is the whole
//! point of the slot.
//!
//! Torpedoes fly in 3D (they rise off the plane and arc down), so the radius
//! test is done on the flattened XY position: the bubble reads as a flat ring on
//! screen, and a torpedo passing directly overhead is very much incoming. The
//! interception message still carries the true 3D point, so the client's tracer
//! reaches where the thing actually was.

use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::components::{Faction, Projectile, Ship};
use crate::emp::EmpBolt;
use crate::events::MunitionIntercepted;
use crate::torpedo::Torpedo;

/// A close-in defensive emitter. Powered from the ship's battery, not a cooldown.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct PointDefense {
    /// How far out the screen reaches.
    pub radius: f32,
    /// Battery spent per second while held.
    pub drain_per_sec: f32,
    /// Interceptions per second. This — not the radius — is what stops the
    /// screen from being an answer to a whole volley at once.
    pub rate: f32,
    /// Seconds until the next interception. Live state.
    #[serde(skip)]
    pub timer: f32,
    /// Set by [`battery_draw_system`](crate::drive::battery_draw_system): held
    /// *and* paid for. Live state.
    #[serde(skip)]
    pub powered: bool,
}

impl Default for PointDefense {
    fn default() -> Self {
        Self {
            // Comfortably outside the hull but well inside a broadside's ~810
            // reach: the screen buys you the last stretch of a shot's flight,
            // not immunity from being shot at.
            radius: 190.0,
            drain_per_sec: 0.8,
            rate: 4.0,
            timer: 0.0,
            powered: false,
        }
    }
}

/// Bevy system: destroy one incoming hostile munition per emitter per interval.
///
/// Runs in `SimSet::Weapons` after `projectile_system`, so a shot that already
/// landed this step is not un-hit by a screen firing afterwards — the defence
/// stops what is still in the air, and nothing more.
///
/// The three munition kinds are three queries rather than one tagged view: they
/// carry their shooter's faction in three unrelated structs, and a shared marker
/// component would have to be attached a step *after* a shot spawns, which costs
/// the screen its reaction to anything fired point-blank.
pub fn point_defense_system(
    time: Res<Time>,
    mut commands: Commands,
    mut intercepted: MessageWriter<MunitionIntercepted>,
    mut emitters: Query<(&Transform, &Faction, &mut PointDefense), With<Ship>>,
    shots: Query<(Entity, &Transform, &Projectile)>,
    bolts: Query<(Entity, &Transform, &EmpBolt)>,
    torpedoes: Query<(Entity, &Transform, &Torpedo)>,
) {
    let dt = time.delta_secs();
    for (transform, faction, mut pd) in &mut emitters {
        pd.timer = (pd.timer - dt).max(0.0);
        if !pd.powered || pd.timer > 0.0 {
            continue;
        }
        let pos = transform.translation.truncate();

        // Nearest first, across all three kinds: the closest thing in the air is
        // the one about to land.
        let mut best: Option<(Entity, Vec3, f32)> = None;
        let mut consider = |entity: Entity, at: Vec3, shooter: Faction| {
            if !faction.hostile_to(shooter) {
                return;
            }
            let dist = pos.distance(at.truncate());
            if dist > pd.radius {
                return;
            }
            if best.is_none_or(|(_, _, nearest)| dist < nearest) {
                best = Some((entity, at, dist));
            }
        };

        for (entity, tf, shot) in &shots {
            consider(entity, tf.translation, shot.faction);
        }
        for (entity, tf, bolt) in &bolts {
            consider(entity, tf.translation, bolt.faction);
        }
        for (entity, tf, torpedo) in &torpedoes {
            consider(entity, tf.translation, torpedo.faction);
        }

        if let Some((entity, at, _)) = best {
            commands.entity(entity).despawn();
            intercepted.write(MunitionIntercepted {
                position: at,
                from: pos,
            });
            pd.timer = 1.0 / pd.rate.max(0.01);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Ttl, Velocity};
    use bevy_app::prelude::*;
    use bevy_math::Vec2;

    /// A world with the screen running and one emitter at the origin.
    fn world_with_screen(powered: bool) -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_message::<MunitionIntercepted>()
            .add_systems(Update, point_defense_system);

        let ship = app
            .world_mut()
            .spawn((
                Ship,
                Faction::Corsairs,
                Transform::default(),
                PointDefense {
                    powered,
                    ..PointDefense::default()
                },
            ))
            .id();
        (app, ship)
    }

    fn shot(app: &mut App, faction: Faction, at: Vec2) -> Entity {
        app.world_mut()
            .spawn((
                Projectile {
                    damage: 1.0,
                    faction,
                    radius: 5.0,
                },
                Velocity(Vec2::ZERO),
                Transform::from_translation(at.extend(0.0)),
                Ttl(5.0),
            ))
            .id()
    }

    #[test]
    fn it_kills_a_hostile_shot_inside_the_radius() {
        let (mut app, _) = world_with_screen(true);
        let incoming = shot(&mut app, Faction::Houses, Vec2::new(100.0, 0.0));
        app.update();
        assert!(app.world().get_entity(incoming).is_err());
    }

    #[test]
    fn and_never_a_friendly_one() {
        let (mut app, _) = world_with_screen(true);
        let mine = shot(&mut app, Faction::Corsairs, Vec2::new(100.0, 0.0));
        app.update();
        assert!(
            app.world().get_entity(mine).is_ok(),
            "the screen must not eat the ship's own broadside"
        );
    }

    #[test]
    fn and_nothing_beyond_the_radius() {
        let (mut app, _) = world_with_screen(true);
        let far = shot(&mut app, Faction::Houses, Vec2::new(1000.0, 0.0));
        app.update();
        assert!(app.world().get_entity(far).is_ok());
    }

    #[test]
    fn and_nothing_at_all_when_unpowered() {
        let (mut app, _) = world_with_screen(false);
        let incoming = shot(&mut app, Faction::Houses, Vec2::new(100.0, 0.0));
        app.update();
        assert!(
            app.world().get_entity(incoming).is_ok(),
            "a flat battery is a screen that is not there"
        );
    }

    /// The rate is what stops the screen answering a whole volley at once: a
    /// second shot in the same step must survive to be taken on the next one.
    #[test]
    fn it_takes_one_munition_per_interval() {
        let (mut app, _) = world_with_screen(true);
        let first = shot(&mut app, Faction::Houses, Vec2::new(50.0, 0.0));
        let second = shot(&mut app, Faction::Houses, Vec2::new(60.0, 0.0));
        app.update();

        let world = app.world();
        let survivors = [first, second]
            .into_iter()
            .filter(|e| world.get_entity(*e).is_ok())
            .count();
        assert_eq!(survivors, 1, "exactly one shot should have been taken");
        // And the nearer one is the one that went.
        assert!(world.get_entity(first).is_err(), "nearest first");
    }

    /// A torpedo flies off the plane, so the radius test must flatten it — one
    /// passing directly overhead is very much incoming.
    #[test]
    fn it_also_takes_torpedoes_arcing_overhead() {
        let (mut app, ship) = world_with_screen(true);
        let fish = app
            .world_mut()
            .spawn((
                Torpedo {
                    faction: Faction::Houses,
                    target: ship,
                    turn_rate: 1.0,
                    speed: 100.0,
                    damage: 10.0,
                    radius: 6.0,
                    vel: Vec3::ZERO,
                },
                Transform::from_translation(Vec3::new(80.0, 0.0, 400.0)),
            ))
            .id();
        app.update();
        assert!(
            app.world().get_entity(fish).is_err(),
            "height off the plane must not put a torpedo out of reach"
        );
    }
}
