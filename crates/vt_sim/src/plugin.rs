//! The [`SimPlugin`] wires every simulation system into a Bevy `App`.
//!
//! All sim systems run in [`FixedUpdate`] so the game steps at a fixed rate
//! independent of render framerate — important for deterministic, replayable
//! ship physics. The client mounts this plugin alongside `DefaultPlugins`;
//! headless tests mount it alongside `bevy_app`'s minimal schedules.

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

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

/// Ordered stages of a single simulation step.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Spawn waves and evaluate win/lose.
    Director,
    /// AI controllers decide their ships' helm and fire orders.
    Ai,
    /// Ship systems: batteries, EMP recovery, speed scaling.
    Systems,
    /// Turn hulls and integrate positions.
    Movement,
    /// Push overlapping hulls apart and resolve rams. After `Movement` because
    /// it corrects the positions movement just wrote, and before `Bounds` so the
    /// boundary spring always gets the last word on where a ship may be.
    Contact,
    /// Keep ships inside the star system's soft boundary.
    Bounds,
    /// Fire broadsides/EMP, launch torpedoes, fly projectiles.
    Weapons,
    /// Steer homing torpedoes.
    Homing,
    /// Resolve hits, apply damage, destroy wrecks.
    Resolution,
    /// Cripple low-hull ships and resolve boarding.
    Piracy,
}

/// Registers the Void & Thunder simulation on a Bevy `App`.
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SystemBounds>()
            // Defaults reproduce the constants exactly; the client may load a
            // RON file over the top (see vt_client's DataPlugin).
            .init_resource::<SimTuning>()
            .init_resource::<SpawnDirector>()
            .init_resource::<Encounter>()
            .init_resource::<Plunder>()
            .init_resource::<BoardIntent>()
            .init_resource::<Boarding>()
            .add_message::<ShipHit>()
            .add_message::<ShipDestroyed>()
            .add_message::<EmpImpact>()
            .configure_sets(
                FixedUpdate,
                (
                    SimSet::Director,
                    SimSet::Ai,
                    SimSet::Systems,
                    SimSet::Movement,
                    SimSet::Contact,
                    SimSet::Bounds,
                    SimSet::Weapons,
                    SimSet::Homing,
                    SimSet::Resolution,
                    SimSet::Piracy,
                )
                    .chain(),
            )
            .add_systems(FixedUpdate, director_system.in_set(SimSet::Director))
            .add_systems(
                FixedUpdate,
                (ai_system, ai_abilities_system).chain().in_set(SimSet::Ai),
            )
            .add_systems(
                FixedUpdate,
                (
                    battery_system,
                    torpedo_reload_system,
                    microwarp_system,
                    speed_scale_system,
                    shield_refit_system,
                    shield_regen_system,
                )
                    .chain()
                    .in_set(SimSet::Systems),
            )
            .add_systems(FixedUpdate, movement_system.in_set(SimSet::Movement))
            .add_systems(FixedUpdate, ram_system.in_set(SimSet::Contact))
            .add_systems(FixedUpdate, bounds_system.in_set(SimSet::Bounds))
            .add_systems(
                FixedUpdate,
                (
                    weapons_system,
                    emp_system,
                    // Drain whatever was queued as of last step *before* this
                    // step's lock/release runs — matches the single-function
                    // predecessor's per-frame ordering (a release this frame
                    // queues for next frame's drain, not an immediate one).
                    torpedo_launch_system,
                    torpedo_lock_system,
                    projectile_system,
                )
                    .chain()
                    .in_set(SimSet::Weapons),
            )
            .add_systems(FixedUpdate, torpedo_homing_system.in_set(SimSet::Homing))
            .add_systems(
                FixedUpdate,
                (
                    collision_system,
                    emp_bolt_system,
                    torpedo_hit_system,
                    destruction_system,
                )
                    .chain()
                    .in_set(SimSet::Resolution),
            )
            .add_systems(
                FixedUpdate,
                (cripple_system, boarding_system)
                    .chain()
                    .in_set(SimSet::Piracy),
            );
    }
}
