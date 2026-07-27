//! Fire barrels: a burning hazard rolled off the stern.
//!
//! Black Flag's answer to being chased. The special slot's third option, and the
//! only one that is not aimed: torpedoes and the microwarp both ask the pilot
//! where, and this one asks *when*. It is what a ship reaches for when something
//! faster is on its tail and the guns will not bear.
//!
//! The rack owns the cadence. [`PilotIntent::barrel_drop`] is a level, not an
//! edge — "I want a barrel astern now" — and [`FireBarrelRack::cooldown`] decides
//! how fast a held key actually pays out. That keeps edge detection in the client
//! where the keyboard is, and means the AI can hold the same flag without having
//! to fake a keypress rhythm.
//!
//! Like torpedoes, barrels are finite and restocked by taking ships, so the
//! special slot is a resource decision either way you fit it.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::combat::{apply_hull_damage, circles_overlap};
use crate::components::{
    Brace, Collider, Faction, Heading, Hull, Invulnerable, PilotIntent, Ship, Ttl,
};
use crate::events::ShipHit;
use crate::shield::{arc_of_hit, Shield};
use crate::tuning::SimTuning;

/// How far astern of the hull centre a barrel is set down. Clear of the ship's
/// own collider so a drop never immediately singes the ship that made it.
const DROP_OFFSET: f32 = 34.0;

/// Seconds of burn a barrel accumulates before it announces a [`ShipHit`].
///
/// A barrel damages continuously, but the juice layer is built for *blows*: one
/// `ShipHit` is a spark burst, a sound and a touch of hitstop. Emitting one per
/// step would be 64 of each per second and would read as the game breaking
/// rather than as a fire. This throttles the announcement without touching the
/// damage, which stays smooth.
const HIT_REPORT_INTERVAL: f32 = 0.25;

/// A rack of fire barrels. Finite, and refilled only by boarding a prize.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct FireBarrelRack {
    /// Barrels aboard right now. Live state — authoring it would refill the rack
    /// on every hot reload.
    #[serde(skip)]
    pub magazine: u32,
    /// How many the rack holds when full.
    pub magazine_max: u32,
    /// Barrels recovered from a boarded prize, drawn uniformly from this range.
    pub resupply_min: u32,
    pub resupply_max: u32,
    /// Seconds between drops while the key is held.
    pub cooldown: f32,
    /// Seconds until the next drop is allowed. Live state.
    #[serde(skip)]
    pub timer: f32,
    /// How long a dropped barrel burns.
    pub ttl: f32,
    /// How wide the fire is.
    pub radius: f32,
    /// Hull damage a ship takes per second inside the fire.
    pub damage_per_sec: f32,
}

impl Default for FireBarrelRack {
    fn default() -> Self {
        Self {
            magazine: 8,
            magazine_max: 8,
            resupply_min: 1,
            resupply_max: 3,
            cooldown: 1.2,
            timer: 0.0,
            // Long enough to seal a lane behind you for a whole turn, short
            // enough that the field is not still burning two passes later.
            ttl: 6.0,
            radius: 52.0,
            damage_per_sec: 14.0,
        }
    }
}

impl FireBarrelRack {
    /// This rack with its magazine full — how a ship enters the field. The
    /// authored file describes the *fit*; the live count is derived from it.
    pub fn stocked(self) -> Self {
        Self {
            magazine: self.magazine_max,
            ..self
        }
    }

    /// Take `n` barrels aboard, up to capacity. Returns how many actually
    /// fitted, so a caller can report a resupply honestly.
    pub fn restock(&mut self, n: u32) -> u32 {
        let room = self.magazine_max.saturating_sub(self.magazine);
        let taken = n.min(room);
        self.magazine += taken;
        taken
    }
}

/// A barrel burning on the plane. Paired with a [`Ttl`], which is what ends it.
#[derive(Component, Clone, Copy, Debug)]
pub struct FireBarrel {
    /// Who dropped it — a ship never burns in its own fire, or a friend's.
    pub faction: Faction,
    pub radius: f32,
    pub damage_per_sec: f32,
    /// Seconds of burning since this barrel last announced a hit. Live state,
    /// and the reason the juice layer sees blows rather than a 64Hz stream.
    pub pending: f32,
}

/// Bevy system: set a barrel down astern when the pilot asks and the rack allows.
pub fn barrel_drop_system(
    time: Res<Time>,
    mut commands: Commands,
    mut ships: Query<(
        &Transform,
        &Heading,
        &Faction,
        &PilotIntent,
        &mut FireBarrelRack,
    )>,
) {
    let dt = time.delta_secs();
    for (transform, heading, faction, intent, mut rack) in &mut ships {
        rack.timer = (rack.timer - dt).max(0.0);
        if !intent.barrel_drop || rack.timer > 0.0 || rack.magazine == 0 {
            continue;
        }
        let astern = transform.translation.truncate() - heading.forward() * DROP_OFFSET;
        commands.spawn((
            FireBarrel {
                faction: *faction,
                radius: rack.radius,
                damage_per_sec: rack.damage_per_sec,
                pending: 0.0,
            },
            Transform::from_translation(astern.extend(0.0)),
            Ttl(rack.ttl),
        ));
        rack.magazine -= 1;
        rack.timer = rack.cooldown;
    }
}

/// Bevy system: burn whatever is standing in a barrel, and expire spent ones.
///
/// Damage goes through [`apply_hull_damage`] like every other blow, so bracing,
/// shields and invulnerability all behave exactly as they do against shot — the
/// one place a hull goes down stays the only place.
#[allow(clippy::type_complexity)]
pub fn barrel_burn_system(
    time: Res<Time>,
    tuning: Res<SimTuning>,
    mut commands: Commands,
    mut hits: MessageWriter<ShipHit>,
    mut barrels: Query<(Entity, &Transform, &mut FireBarrel, &mut Ttl)>,
    mut ships: Query<
        (
            Entity,
            &Transform,
            &Heading,
            &Faction,
            &Collider,
            &mut Hull,
            Option<&mut Shield>,
            Option<&Brace>,
            Has<Invulnerable>,
        ),
        With<Ship>,
    >,
) {
    let dt = time.delta_secs();
    for (entity, barrel_tf, mut barrel, mut ttl) in &mut barrels {
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let at = barrel_tf.translation.truncate();

        // Announce at a fixed cadence rather than per step. The damage below is
        // applied every step regardless — only the *report* is throttled.
        barrel.pending += dt;
        let announce = barrel.pending >= HIT_REPORT_INTERVAL;
        if announce {
            barrel.pending = 0.0;
        }

        for (ship, ship_tf, heading, faction, collider, mut hull, shield, brace, invulnerable) in
            &mut ships
        {
            if !barrel.faction.hostile_to(*faction) {
                continue;
            }
            let ship_pos = ship_tf.translation.truncate();
            if !circles_overlap(at, barrel.radius, ship_pos, collider.radius) {
                continue;
            }
            let report = apply_hull_damage(
                &mut hull,
                shield.map(Mut::into_inner),
                arc_of_hit(ship_tf, heading, at),
                barrel.damage_per_sec * dt,
                brace.is_some_and(|b| b.active),
                invulnerable,
                tuning.brace_damage_factor,
            );
            if announce {
                hits.write(ShipHit {
                    position: at,
                    ship,
                    faction: *faction,
                    // The report covers one step, but the blow the player is
                    // being shown covers the whole interval — scale it so the
                    // spark burst matches how hard the fire is actually biting.
                    damage: barrel.damage_per_sec * HIT_REPORT_INTERVAL,
                    // Fire has no travel direction, so sparks spray away from
                    // the flames rather than back along a trajectory.
                    direction: (ship_pos - at).normalize_or(Vec2::Y),
                    report,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Faction, ShipStats};
    use crate::spawn::ship_bundle;
    use bevy_app::prelude::*;
    use bevy_math::Vec2;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<SimTuning>()
            .add_message::<ShipHit>()
            .add_systems(Update, (barrel_drop_system, barrel_burn_system).chain());
        app
    }

    fn step(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    fn dropper(app: &mut App, holding: bool, rack: FireBarrelRack) -> Entity {
        app.world_mut()
            .spawn((
                ship_bundle(
                    Faction::Corsairs,
                    ShipStats::default(),
                    100.0,
                    Vec2::ZERO,
                    0.0, // bow along +X, so astern is -X
                    Default::default(),
                ),
                rack,
            ))
            // The bundle already carries a `PilotIntent`; overwrite it rather
            // than spawning a second one, which Bevy rejects outright.
            .insert(PilotIntent {
                barrel_drop: holding,
                ..PilotIntent::default()
            })
            .id()
    }

    #[test]
    fn a_drop_spends_a_barrel_and_lands_astern() {
        let mut app = app();
        let ship = dropper(&mut app, true, FireBarrelRack::default().stocked());
        step(&mut app, 0.1);

        assert_eq!(
            app.world().get::<FireBarrelRack>(ship).unwrap().magazine,
            7,
            "the drop must cost a barrel"
        );
        let mut barrels = app.world_mut().query::<(&FireBarrel, &Transform)>();
        let (_, tf) = barrels.iter(app.world()).next().expect("a barrel");
        assert!(
            tf.translation.x < 0.0,
            "a barrel goes over the stern, not the bow"
        );
    }

    #[test]
    fn an_empty_rack_drops_nothing() {
        let mut app = app();
        dropper(
            &mut app,
            true,
            FireBarrelRack {
                magazine: 0,
                ..FireBarrelRack::default()
            },
        );
        step(&mut app, 0.1);

        let mut barrels = app.world_mut().query::<&FireBarrel>();
        assert_eq!(barrels.iter(app.world()).count(), 0);
    }

    /// The rack owns the cadence, so holding the key down pays out at
    /// `cooldown`, not once per step.
    #[test]
    fn holding_the_key_drops_at_the_racks_own_rate() {
        let mut app = app();
        dropper(&mut app, true, FireBarrelRack::default().stocked());
        for _ in 0..5 {
            step(&mut app, 0.1);
        }

        let mut barrels = app.world_mut().query::<&FireBarrel>();
        assert_eq!(
            barrels.iter(app.world()).count(),
            1,
            "half a second at a 1.2s cooldown is one barrel, not five"
        );
    }

    #[test]
    fn a_barrel_burns_a_pursuer_and_not_the_ship_that_dropped_it() {
        let mut app = app();
        let dropper = dropper(&mut app, true, FireBarrelRack::default().stocked());
        // A hostile sitting right where the barrel will land.
        let chaser = app
            .world_mut()
            .spawn(ship_bundle(
                Faction::Houses,
                ShipStats::default(),
                100.0,
                Vec2::new(-DROP_OFFSET, 0.0),
                0.0,
                Default::default(),
            ))
            .id();

        for _ in 0..10 {
            step(&mut app, 0.1);
        }

        let world = app.world();
        assert!(
            world.get::<Hull>(chaser).unwrap().current < 100.0,
            "a hostile standing in the fire must burn"
        );
        assert_eq!(
            world.get::<Hull>(dropper).unwrap().current,
            100.0,
            "and the ship that lit it must not"
        );
    }

    #[test]
    fn a_barrel_expires() {
        let mut app = app();
        let ship = dropper(&mut app, true, FireBarrelRack::default().stocked());
        step(&mut app, 0.1);
        // Let go, or the rack keeps paying out and we would be measuring the
        // cadence rather than the burn.
        app.world_mut()
            .get_mut::<PilotIntent>(ship)
            .unwrap()
            .barrel_drop = false;
        for _ in 0..10 {
            step(&mut app, 1.0);
        }

        let mut barrels = app.world_mut().query::<&FireBarrel>();
        assert_eq!(
            barrels.iter(app.world()).count(),
            0,
            "a 6s barrel must not still be burning after ten"
        );
    }

    #[test]
    fn an_invulnerable_ship_does_not_burn() {
        let mut app = app();
        dropper(&mut app, true, FireBarrelRack::default().stocked());
        let target = app
            .world_mut()
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::new(-DROP_OFFSET, 0.0),
                    0.0,
                    Default::default(),
                ),
                Invulnerable,
            ))
            .id();

        for _ in 0..10 {
            step(&mut app, 0.1);
        }

        assert_eq!(app.world().get::<Hull>(target).unwrap().current, 100.0);
    }

    /// The fire damages every step but must only *announce* on the interval, or
    /// the juice layer gets 64 spark bursts and hitstops a second.
    #[test]
    fn burning_announces_far_less_often_than_it_damages() {
        let mut app = app();
        dropper(&mut app, true, FireBarrelRack::default().stocked());
        app.world_mut().spawn(ship_bundle(
            Faction::Houses,
            ShipStats::default(),
            100.0,
            Vec2::new(-DROP_OFFSET, 0.0),
            0.0,
            Default::default(),
        ));

        // One second of burning at a 64Hz-ish step.
        let mut announced = 0;
        for _ in 0..64 {
            step(&mut app, 1.0 / 64.0);
            announced += app
                .world()
                .resource::<Messages<ShipHit>>()
                .iter_current_update_messages()
                .count();
        }

        assert!(
            (3..=6).contains(&announced),
            "a second of fire at a {HIT_REPORT_INTERVAL}s interval should announce \
             about four times, got {announced}"
        );
    }
}
