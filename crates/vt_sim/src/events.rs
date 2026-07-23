//! Simulation events the presentation layer reacts to.
//!
//! The sim stays authoritative and renderer-agnostic, but it announces the
//! moments worth dressing up — a hull taking a hit, a ship being destroyed — so
//! the client can throw sparks, explosions and screen shake without owning any
//! rules. Positions are world-space.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;

use crate::components::Faction;

/// A cannonball struck a ship's hull.
#[derive(Message, Clone, Copy, Debug)]
pub struct ShipHit {
    pub position: Vec2,
    /// The faction of the ship that was hit.
    pub faction: Faction,
}

/// A ship was destroyed (hull reduced to zero).
#[derive(Message, Clone, Copy, Debug)]
pub struct ShipDestroyed {
    pub position: Vec2,
    /// The faction of the ship that was destroyed.
    pub faction: Faction,
}
