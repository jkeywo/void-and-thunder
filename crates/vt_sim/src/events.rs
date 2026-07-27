//! Simulation events the presentation layer reacts to.
//!
//! The sim stays authoritative and renderer-agnostic, but it announces the
//! moments worth dressing up — a hull taking a hit, a ship being destroyed — so
//! the client can throw sparks, explosions and screen shake without owning any
//! rules. Positions are world-space.

use bevy_ecs::prelude::*;
use bevy_math::{Vec2, Vec3};

use crate::components::Faction;
use crate::shield::DamageReport;

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
    /// Unit vector the blow was travelling along. The client sprays sparks back
    /// against this, so a hit reads as having come from *somewhere* rather than
    /// blooming symmetrically out of the hull. Every writer already knows it —
    /// a shot has a velocity, a ram has a contact normal — so it costs nothing
    /// to carry, and guessing it from the geometry downstream would be wrong for
    /// a glancing blow.
    pub direction: Vec2,
    /// How the blow actually split between shield and hull, after bracing. A
    /// shield turning a shot aside and a shot breaching the hull are different
    /// events to the player and want different colours and weight, and only the
    /// damage resolution knows which happened.
    pub report: DamageReport,
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
    /// Where the bow was pointing, and how fast it was going, at the moment it
    /// died. Carried because the entity is gone by the time anyone reads this:
    /// without them a client cannot show a wreck that carries on along the
    /// course the ship was making, and a corpse that stops dead on the spot
    /// looks like the ship was deleted rather than killed.
    pub heading: f32,
    pub velocity: Vec2,
}

/// An EMP bolt struck a ship.
#[derive(Message, Clone, Copy, Debug)]
pub struct EmpImpact {
    pub position: Vec2,
}

/// A point-defence screen swatted something out of the air.
///
/// The sim spawns no tracer of its own: this says where the kill happened and
/// where it was fired from, and the client draws the line. Carrying `from` saves
/// the presentation layer a lookup of a ship it would have to find by position,
/// and the emitter may well have moved by the time it reads this.
#[derive(Message, Clone, Copy, Debug)]
pub struct MunitionIntercepted {
    /// Where the munition was, in full 3D — a torpedo dies well off the plane.
    pub position: Vec3,
    /// The emitter that took it.
    pub from: Vec2,
}
