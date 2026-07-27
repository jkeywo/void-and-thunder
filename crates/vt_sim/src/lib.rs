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
pub mod collide;
pub mod combat;
pub mod components;
pub mod drive;
pub mod emp;
pub mod events;
pub mod harness;
pub mod pilot;
pub mod piracy;
pub mod plugin;
pub mod shield;
pub mod ship;
pub mod spawn;
pub mod torpedo;
pub mod tuning;
pub mod util;
pub mod world;

pub use plugin::{SimPlugin, SimSet};

/// Common imports for consumers of the simulation.
pub mod prelude {
    pub use crate::ai::desired_helm;
    pub use crate::collide::{ram_damage, RAM_DAMAGE_THRESHOLD};
    pub use crate::combat::{
        apply_hull_damage, broadside_direction, broadside_volley, intercept_lead, BankState,
        Broadside, Lead, ProjectileSpawn, PROJECTILE_TTL,
    };
    pub use crate::components::{
        AiController, Anchored, AngularVelocity, Brace, ClassId, Collider, Disabled, EmpDefense,
        Faction, FireOrders, Heading, Helm, Hull, Invulnerable, Landmark, PilotIntent, Projectile,
        Protagonist, Ship, ShipStats, SpeedScale, Ttl, Velocity, ENGAGEMENT_RANGE,
    };
    pub use crate::drive::{clamp_to_range, speed_scale, BoostDrive, MicrowarpDrive};
    pub use crate::emp::{EmpBolt, EmpWeapon};
    pub use crate::events::{EmpImpact, ShipDestroyed, ShipHit};
    pub use crate::harness::Harness;
    pub use crate::pilot::{Action, Contact, Kit, PilotBrain, Plan, Situation};
    pub use crate::piracy::{
        BoardIntent, Boarding, Plunder, BOARD_DWELL, BOARD_RANGE, CRIPPLE_THRESHOLD,
    };
    pub use crate::plugin::{SimPlugin, SimSet};
    pub use crate::shield::{shield_arc, DamageReport, Shield, ShieldArc, ShieldBank};
    pub use crate::ship::{agility_at, helm_step};
    pub use crate::spawn::{
        reset_encounter, ship_bundle, DirectorSettings, Encounter, FinaleWave, Outcome,
        ShipLoadout, SpawnDirector,
    };
    pub use crate::torpedo::{Torpedo, TorpedoBay, TorpedoLaunchQueue, TorpedoLock};
    pub use crate::tuning::{AiTuning, PilotTuning, SimTuning};
    pub use crate::util::{lcg_next, wrap_angle};
    pub use crate::world::{bounds_return, SystemBounds};
}
