//! The [`SimPlugin`] wires every simulation system into a Bevy `App`.
//!
//! All sim systems run in [`FixedUpdate`] so the game steps at a fixed rate
//! independent of render framerate — important for deterministic, replayable
//! ship physics. The client mounts this plugin alongside `DefaultPlugins`;
//! headless tests mount it alongside `bevy_app`'s minimal schedules.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

use crate::ai::ai_system;
use crate::combat::{collision_system, destruction_system, projectile_system, weapons_system};
use crate::ship::movement_system;
use crate::spawn::{director_system, Encounter, SpawnDirector};
use crate::world::{bounds_system, SystemBounds};

/// Ordered stages of a single simulation step.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Spawn waves and evaluate win/lose.
    Director,
    /// AI controllers decide their ships' helm and fire orders.
    Ai,
    /// Turn hulls and integrate positions.
    Movement,
    /// Keep ships inside the star system's soft boundary.
    Bounds,
    /// Fire broadsides, fly cannonballs.
    Weapons,
    /// Resolve hits, apply damage, destroy wrecks.
    Resolution,
}

/// Registers the Void & Thunder simulation on a Bevy `App`.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SystemBounds>()
            .init_resource::<SpawnDirector>()
            .init_resource::<Encounter>()
            .configure_sets(
                FixedUpdate,
                (
                    SimSet::Director,
                    SimSet::Ai,
                    SimSet::Movement,
                    SimSet::Bounds,
                    SimSet::Weapons,
                    SimSet::Resolution,
                )
                    .chain(),
            )
            .add_systems(FixedUpdate, director_system.in_set(SimSet::Director))
            .add_systems(FixedUpdate, ai_system.in_set(SimSet::Ai))
            .add_systems(FixedUpdate, movement_system.in_set(SimSet::Movement))
            .add_systems(FixedUpdate, bounds_system.in_set(SimSet::Bounds))
            .add_systems(
                FixedUpdate,
                (weapons_system, projectile_system)
                    .chain()
                    .in_set(SimSet::Weapons),
            )
            .add_systems(
                FixedUpdate,
                (collision_system, destruction_system)
                    .chain()
                    .in_set(SimSet::Resolution),
            );
    }
}
