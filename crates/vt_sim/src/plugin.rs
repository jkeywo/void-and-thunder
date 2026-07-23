//! The [`SimPlugin`] wires every simulation system into a Bevy `App`.
//!
//! All sim systems run in [`FixedUpdate`] so the game steps at a fixed rate
//! independent of render framerate — important for deterministic, replayable
//! ship physics. The client mounts this plugin alongside `DefaultPlugins`;
//! headless tests mount it alongside `bevy_app`'s minimal schedules.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

use crate::combat::{collision_system, destruction_system, projectile_system, weapons_system};
use crate::ship::movement_system;

/// Ordered stages of a single simulation step.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Turn hulls and integrate positions.
    Movement,
    /// Fire broadsides, fly cannonballs.
    Weapons,
    /// Resolve hits, apply damage, destroy wrecks.
    Resolution,
}

/// Registers the Void & Thunder simulation on a Bevy `App`.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            FixedUpdate,
            (SimSet::Movement, SimSet::Weapons, SimSet::Resolution).chain(),
        )
        .add_systems(FixedUpdate, movement_system.in_set(SimSet::Movement))
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
