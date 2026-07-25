//! Simulation events the presentation layer reacts to.
//!
//! The sim stays authoritative and renderer-agnostic, but it announces the
//! moments worth dressing up — a hull taking a hit, a ship being destroyed — so
//! the client can throw sparks, explosions and screen shake without owning any
//! rules. Positions are world-space.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;

use crate::components::Faction;

/// A cannonball or torpedo struck a ship's hull.
///
/// `ship` names the hull that was struck so the presentation layer can tell
/// *whose* damage this is — the client shakes the camera hard for the player's
/// own hits and only faintly for distant ones — and `damage` is the blow's
/// weight *before* bracing, so a torpedo lands harder than a cannonball.
#[derive(Message, Clone, Copy, Debug)]
pub struct ShipHit {
    pub position: Vec2,
    /// The ship that was hit.
    pub ship: Entity,
    /// The faction of the ship that was hit.
    pub faction: Faction,
    /// Damage the blow carried, before brace reduction.
    pub damage: f32,
}

/// A ship was destroyed (hull reduced to zero).
#[derive(Message, Clone, Copy, Debug)]
pub struct ShipDestroyed {
    pub position: Vec2,
    /// The ship that was destroyed. Already despawned by the time a reader sees
    /// this, so it is only good for identity comparisons ("was that me?").
    pub ship: Entity,
    /// The faction of the ship that was destroyed.
    pub faction: Faction,
}

/// An EMP bolt struck a ship.
#[derive(Message, Clone, Copy, Debug)]
pub struct EmpImpact {
    pub position: Vec2,
}
