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
use crate::combat::{collision_system, destruction_system, projectile_system, weapons_system};
use crate::drive::{battery_system, microwarp_system, speed_scale_system};
use crate::emp::{emp_bolt_system, emp_system};
use crate::events::{EmpImpact, ShipDestroyed, ShipHit};
use crate::piracy::{boarding_system, cripple_system, BoardIntent, Boarding, Plunder};
use crate::ship::movement_system;
use crate::spawn::{director_system, Encounter, SpawnDirector};
use crate::torpedo::{
    torpedo_aim_system, torpedo_hit_system, torpedo_homing_system, torpedo_reload_system,
};
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
                )
                    .chain(),
                movement_system,
                bounds_system,
                (
                    weapons_system,
                    emp_system,
                    torpedo_aim_system,
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
    use crate::components::{AiController, EmpDefense, Faction, Hull, Protagonist, ShipStats};
    use crate::spawn::{ship_bundle, ShipLoadout};
    use bevy_math::Vec2;

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
}
