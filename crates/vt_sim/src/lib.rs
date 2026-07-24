//! # Void & Thunder — Simulation
//!
//! The renderer-agnostic core of the game: ship physics and ship-to-ship
//! combat in the void lanes of the Settled Dark, built on Bevy's ECS.
//!
//! This crate depends only on the *logic* parts of Bevy (`bevy_ecs`,
//! `bevy_app`, `bevy_math`, `bevy_time`, `bevy_transform`) — no windowing,
//! rendering or audio — so it compiles fast and runs headless in tests. The
//! `vt_client` crate supplies the renderer, input and window, and mounts
//! [`SimPlugin`] to bring the simulation to life.
//!
//! ## Shape of a ship
//!
//! A ship is an entity with: [`Ship`], [`Transform`](bevy_transform::components::Transform)
//! (position), [`Heading`], [`Velocity`], [`ShipStats`], [`Helm`] (control
//! intent), [`Hull`], [`Collider`], [`Faction`], plus one [`Broadside`] and a
//! [`FireOrders`]. A controller (the player, or later an AI) writes `Helm` and
//! `FireOrders`; the sim does the rest.

pub mod ai;
pub mod combat;
pub mod components;
pub mod drive;
pub mod emp;
pub mod events;
pub mod harness;
pub mod piracy;
pub mod plugin;
pub mod ship;
pub mod spawn;
pub mod torpedo;
pub mod world;

pub use plugin::{SimPlugin, SimSet};

/// Common imports for consumers of the simulation.
pub mod prelude {
    pub use crate::ai::desired_helm;
    pub use crate::combat::{broadside_direction, broadside_volley, ProjectileSpawn};
    pub use crate::components::{
        AiController, BoostDrive, Brace, Broadside, Collider, Disabled, EmpBolt, EmpDefense,
        EmpWeapon, Faction, FireOrders, Heading, Helm, Hull, Landmark, MicrowarpDrive, PilotIntent,
        Projectile, Protagonist, Ship, ShipStats, SpeedScale, Torpedo, TorpedoBay, Ttl, Velocity,
    };
    pub use crate::drive::{clamp_to_range, speed_scale};
    pub use crate::events::{EmpImpact, ShipDestroyed, ShipHit};
    pub use crate::harness::Harness;
    pub use crate::piracy::{BoardIntent, Plunder, BOARD_RANGE, CRIPPLE_THRESHOLD};
    pub use crate::plugin::{SimPlugin, SimSet};
    pub use crate::spawn::{
        reset_encounter, ship_bundle, Encounter, Outcome, ShipLoadout, SpawnDirector,
    };
    pub use crate::world::{bounds_return, SystemBounds};
}
