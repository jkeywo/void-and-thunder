//! A headless simulation harness for automated tests and tools.
//!
//! [`Harness`] builds a `World` with every sim resource and a `Schedule` that
//! runs one full simulation step in the same order as [`SimPlugin`](crate::SimPlugin),
//! but deterministically — you call [`Harness::step`] with a fixed `dt` instead
//! of relying on real time. Spawn ships (a [`Protagonist`] with an
//! [`AiController`] gives you an AI-piloted *player* ship, since ships are one
//! uniform entity shape), step, and inspect the world.

use bevy_ecs::prelude::*;
use bevy_time::Time;
use std::time::Duration;

use crate::ai::{ai_abilities_system, ai_system};
use crate::collide::ram_system;
use crate::combat::{collision_system, destruction_system, projectile_system, weapons_system};
use crate::drive::{battery_system, microwarp_system, speed_scale_system};
use crate::emp::{emp_bolt_system, emp_system};
use crate::events::{EmpImpact, ShipDestroyed, ShipHit};
use crate::piracy::{boarding_system, cripple_system, BoardIntent, Boarding, Plunder};
use crate::shield::{shield_refit_system, shield_regen_system};
use crate::ship::movement_system;
use crate::spawn::{director_system, Encounter, SpawnDirector};
use crate::torpedo::{
    torpedo_hit_system, torpedo_homing_system, torpedo_launch_system, torpedo_lock_system,
    torpedo_reload_system,
};
use crate::tuning::SimTuning;
use crate::world::{bounds_system, SystemBounds};

/// A deterministic, headless instance of the simulation.
pub struct Harness {
    pub world: World,
    schedule: Schedule,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// Build a fresh world with all sim resources and the step schedule.
    pub fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<SystemBounds>();
        // The defaults are the constants, so the harness needs no data file.
        world.init_resource::<SimTuning>();
        world.init_resource::<SpawnDirector>();
        world.init_resource::<Encounter>();
        world.init_resource::<Plunder>();
        world.init_resource::<BoardIntent>();
        world.init_resource::<Boarding>();
        world.init_resource::<Messages<ShipHit>>();
        world.init_resource::<Messages<ShipDestroyed>>();
        world.init_resource::<Messages<EmpImpact>>();

        let mut schedule = Schedule::default();
        // Mirrors SimPlugin's SimSet ordering, as one linear chain.
        schedule.add_systems(
            (
                (drain_hits, drain_destroyed, drain_emp).chain(),
                director_system,
                (ai_system, ai_abilities_system).chain(),
                (
                    battery_system,
                    torpedo_reload_system,
                    microwarp_system,
                    speed_scale_system,
                    shield_refit_system,
                    shield_regen_system,
                )
                    .chain(),
                movement_system,
                ram_system,
                bounds_system,
                (
                    weapons_system,
                    emp_system,
                    torpedo_launch_system,
                    torpedo_lock_system,
                    projectile_system,
                )
                    .chain(),
                torpedo_homing_system,
                (
                    collision_system,
                    emp_bolt_system,
                    torpedo_hit_system,
                    destruction_system,
                )
                    .chain(),
                (cripple_system, boarding_system).chain(),
            )
                .chain(),
        );

        Self { world, schedule }
    }

    /// Advance the simulation by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        self.world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(dt));
        self.schedule.run(&mut self.world);
    }

    /// Advance by `steps` fixed ticks of `dt` seconds each.
    pub fn run(&mut self, steps: u32, dt: f32) {
        for _ in 0..steps {
            self.step(dt);
        }
    }
}

// Age each message buffer once per step so writers don't leak (the real app's
// `add_message` installs equivalent systems).
fn drain_hits(mut m: ResMut<Messages<ShipHit>>) {
    m.update();
}
fn drain_destroyed(mut m: ResMut<Messages<ShipDestroyed>>) {
    m.update();
}
fn drain_emp(mut m: ResMut<Messages<EmpImpact>>) {
    m.update();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{
        AiController, Anchored, Collider, EmpDefense, Faction, Helm, Hull, Invulnerable,
        Protagonist, ShipStats, Velocity,
    };
    use crate::shield::Shield;
    use crate::spawn::{ship_bundle, ShipLoadout};
    use bevy_math::Vec2;
    use bevy_transform::components::Transform;

    /// The core of the request: a *player-shaped* ship (Protagonist + full
    /// loadout) driven by the AI instead of the client. It should pilot itself
    /// and land hits on an enemy — the only difference from a real player ship
    /// is that `ai_system`, not the client, writes its intent.
    #[test]
    fn ai_can_pilot_a_player_shaped_ship() {
        let mut h = Harness::new();

        // An AI-piloted player ship at the origin.
        h.world.spawn((
            ship_bundle(
                Faction::Corsairs,
                ShipStats::default(),
                100.0,
                Vec2::ZERO,
                0.0,
                ShipLoadout::default(),
            ),
            Protagonist,
            AiController::default(),
        ));
        // A House ship close enough to fight.
        let enemy = h
            .world
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::new(280.0, 0.0),
                    0.0,
                    ShipLoadout::enemy(),
                ),
                AiController::default(),
            ))
            .id();

        h.run(1600, 1.0 / 64.0); // ~25 s

        // Only the (AI-piloted) Corsair can damage the House ship, so if the
        // enemy took damage or was destroyed, the AI pilot did the shooting.
        let engaged = match h.world.get::<Hull>(enemy) {
            Some(hull) => hull.current < hull.max,
            None => true,
        };
        assert!(
            engaged,
            "the AI-piloted player ship should have hit the enemy"
        );
    }

    /// The ability AI (`AiController::piloting`) should EMP a close target — the
    /// enemy's EMP bar takes damage.
    #[test]
    fn ai_pilot_emps_a_close_target() {
        let mut h = Harness::new();
        h.world.spawn((
            ship_bundle(
                Faction::Corsairs,
                ShipStats::default(),
                100.0,
                Vec2::ZERO,
                0.0,
                ShipLoadout::player(),
            ),
            Protagonist,
            AiController::piloting(),
        ));
        // A House ship dead ahead, within EMP range.
        let enemy = h
            .world
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::new(300.0, 0.0),
                    0.0,
                    ShipLoadout::enemy(),
                ),
                AiController::default(),
            ))
            .id();

        h.run(400, 1.0 / 64.0); // ~6 s

        let emped = match h.world.get::<EmpDefense>(enemy) {
            Some(emp) => emp.damage > 0.0,
            None => true,
        };
        assert!(emped, "the AI pilot should EMP a targetable ship");
    }

    /// Hulls are solid. Two ships driven into the same spot must end up apart,
    /// not overlapping — the "ships pass through each other" era is over.
    #[test]
    fn hulls_do_not_pass_through_each_other() {
        let mut h = Harness::new();
        // Start them well inside one another to prove the separation runs at all.
        let a = spawn_drifting(&mut h, Faction::Corsairs, Vec2::ZERO, Vec2::new(120.0, 0.0));
        let b = spawn_drifting(
            &mut h,
            Faction::Houses,
            Vec2::new(20.0, 0.0),
            Vec2::new(-120.0, 0.0),
        );

        h.run(64, 1.0 / 64.0); // 1 s

        let gap = position(&h, a).distance(position(&h, b));
        let reach = Collider::default().radius * 2.0;
        assert!(
            gap >= reach * 0.9,
            "the hulls should have pushed apart to ~{reach}, gap was {gap}"
        );
    }

    /// A ram is a weapon. Driving two hulls together hard costs both of them
    /// hull, and it does so through the ordinary damage path.
    #[test]
    fn a_hard_ram_damages_both_ships() {
        let mut h = Harness::new();
        let a = spawn_drifting(
            &mut h,
            Faction::Corsairs,
            Vec2::new(-40.0, 0.0),
            Vec2::new(150.0, 0.0),
        );
        let b = spawn_drifting(
            &mut h,
            Faction::Houses,
            Vec2::new(40.0, 0.0),
            Vec2::new(-150.0, 0.0),
        );

        h.run(64, 1.0 / 64.0);

        for (entity, who) in [(a, "the rammer"), (b, "the rammed")] {
            let hull = h.world.get::<Hull>(entity).expect("still afloat");
            assert!(
                hull.current < hull.max,
                "{who} should have taken ram damage, hull was {}",
                hull.current
            );
        }
    }

    /// Drifting into someone is not an attack. A slow contact separates the pair
    /// without costing either of them a point of hull, so a scrum of ships
    /// jostling for position does not grind itself down.
    #[test]
    fn a_slow_bump_costs_no_hull() {
        let mut h = Harness::new();
        let a = spawn_drifting(
            &mut h,
            Faction::Corsairs,
            Vec2::new(-30.0, 0.0),
            Vec2::new(8.0, 0.0),
        );
        let b = spawn_drifting(
            &mut h,
            Faction::Houses,
            Vec2::new(30.0, 0.0),
            Vec2::new(-8.0, 0.0),
        );

        h.run(256, 1.0 / 64.0); // 4 s — plenty of time to drift together

        for entity in [a, b] {
            let hull = h.world.get::<Hull>(entity).expect("still afloat");
            assert_eq!(hull.current, hull.max, "a nudge must be free");
        }
    }

    /// An anchored ship is scenery: it absorbs the shove rather than being moved
    /// by it, so a test-range target stays exactly where it was placed even when
    /// something drives into it.
    #[test]
    fn an_anchored_ship_is_not_shoved() {
        let mut h = Harness::new();
        spawn_drifting(
            &mut h,
            Faction::Corsairs,
            Vec2::new(-40.0, 0.0),
            Vec2::new(150.0, 0.0),
        );
        let post = h
            .world
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::ZERO,
                    0.0,
                    ShipLoadout::default(),
                ),
                Anchored,
            ))
            .id();

        h.run(64, 1.0 / 64.0);

        assert_eq!(
            position(&h, post),
            Vec2::ZERO,
            "an anchored hull must hold its mark"
        );
    }

    /// Shields stand between a blow and the hull, and only on the side that took
    /// it. Driven through the full schedule so the whole chain — contact, arc
    /// selection, absorption — is exercised the way the game runs it.
    #[test]
    fn a_shielded_hull_is_spared_on_the_side_that_holds() {
        let mut h = Harness::new();
        let shielded = ShipLoadout {
            shield: Shield {
                max: 500.0, // far more than the ram can spend, so nothing leaks
                ..Default::default()
            },
            ..ShipLoadout::default()
        };
        // Bow-on into an anchored post: the blow lands forward.
        let rammer = h
            .world
            .spawn(ship_bundle(
                Faction::Corsairs,
                ShipStats::default(),
                100.0,
                Vec2::new(-40.0, 0.0),
                0.0,
                shielded,
            ))
            .id();
        h.world.get_mut::<Velocity>(rammer).unwrap().0 = Vec2::new(150.0, 0.0);
        h.world.spawn((
            ship_bundle(
                Faction::Houses,
                ShipStats::default(),
                100.0,
                Vec2::ZERO,
                0.0,
                ShipLoadout::default(),
            ),
            Anchored,
        ));

        h.run(64, 1.0 / 64.0);

        let hull = h.world.get::<Hull>(rammer).expect("still afloat");
        assert_eq!(
            hull.current, hull.max,
            "the fore shield should have taken the whole blow"
        );
        let shield = h.world.get::<Shield>(rammer).expect("still fitted");
        assert!(
            shield.fore.charge < shield.max,
            "and paid for it: fore was {} of {}",
            shield.fore.charge,
            shield.max
        );
        assert_eq!(
            shield.aft.charge, shield.max,
            "while the stern bank is untouched"
        );
    }

    /// A ship with no controller, given an initial velocity — the plain physics
    /// body these contact tests need.
    fn spawn_drifting(h: &mut Harness, faction: Faction, pos: Vec2, vel: Vec2) -> Entity {
        let entity = h
            .world
            .spawn(ship_bundle(
                faction,
                ShipStats::default(),
                100.0,
                pos,
                0.0,
                ShipLoadout::default(),
            ))
            .id();
        h.world.get_mut::<Velocity>(entity).expect("has velocity").0 = vel;
        entity
    }

    fn position(h: &Harness, entity: Entity) -> Vec2 {
        h.world
            .get::<Transform>(entity)
            .expect("still spawned")
            .translation
            .truncate()
    }

    /// The test range's whole premise: something you can shoot at indefinitely
    /// while tuning. An AI-piloted attacker empties its kit into an invulnerable
    /// target for ~25s and the hull must not move a point.
    #[test]
    fn an_invulnerable_ship_takes_no_damage() {
        let mut h = Harness::new();
        h.world.spawn((
            ship_bundle(
                Faction::Corsairs,
                ShipStats::default(),
                100.0,
                Vec2::ZERO,
                0.0,
                ShipLoadout::player(),
            ),
            Protagonist,
            AiController::piloting(),
        ));
        let target = h
            .world
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    Vec2::new(220.0, 0.0),
                    0.0,
                    ShipLoadout::enemy(),
                ),
                Invulnerable,
                Anchored,
            ))
            .id();

        h.run(1600, 1.0 / 64.0); // ~25 s — long enough for broadsides and torpedoes

        let hull = h
            .world
            .get::<Hull>(target)
            .expect("an invulnerable ship is never destroyed");
        assert_eq!(
            hull.current, hull.max,
            "an invulnerable ship must not lose hull"
        );
    }

    /// An anchored ship holds its mark even with the helm hard over — the point
    /// being that nothing (a stray AI write, a design panel) can nudge a target
    /// mid-measurement.
    #[test]
    fn an_anchored_ship_never_moves() {
        let mut h = Harness::new();
        let start = Vec2::new(120.0, -40.0);
        let heading = std::f32::consts::FRAC_PI_2;
        let ship = h
            .world
            .spawn((
                ship_bundle(
                    Faction::Houses,
                    ShipStats::default(),
                    100.0,
                    start,
                    heading,
                    ShipLoadout::enemy(),
                ),
                Anchored,
            ))
            .id();
        // Full throttle, hard over: without `Anchored` this would sail away.
        *h.world.get_mut::<Helm>(ship).unwrap() = Helm {
            throttle: 1.0,
            turn: 1.0,
        };

        h.run(600, 1.0 / 64.0);

        let tf = h.world.get::<Transform>(ship).unwrap();
        assert!(
            tf.translation.truncate().distance(start) < 1e-3,
            "anchored ship drifted to {:?}",
            tf.translation
        );
        // `movement_system` is the only thing that writes rotation from Heading,
        // and it skips anchored ships — so the spawn must have set both.
        let (_, _, yaw) = tf.rotation.to_euler(bevy_math::EulerRot::XYZ);
        assert!(
            (yaw - heading).abs() < 1e-4,
            "anchored ship should render on its authored heading, yaw was {yaw}"
        );
    }
}
