//! ECS components and shared value types for the Void & Thunder simulation.
//!
//! The sim is renderer-agnostic: a ship is an entity carrying a [`Transform`]
//! (position, from `bevy_transform`), a [`Heading`], a [`Velocity`], its
//! [`ShipStats`], a [`Helm`] (control intent), [`Hull`] (health) and one or
//! more [`Broadside`] weapon banks. The client crate adds a `Sprite` to the
//! same entities to draw them.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;

/// Which power a ship answers to. Used for friendly-fire filtering and, later,
/// AI target selection. Names are drawn from the Settled Dark setting.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faction {
    /// The player and allied pirates working the void lanes.
    Corsairs,
    /// House patrol ships enforcing dynastic control of the lanes.
    Houses,
    /// Psychically conditioned Janissariat warships.
    Janissariat,
    /// Ductus Guild couriers and their escorts.
    Guild,
    /// Unaligned freebooters — hostile to everyone, including the player.
    Freebooters,
}

impl Faction {
    /// True when `self` should be able to damage `other`. Same-faction fire is
    /// ignored; freebooters fight everyone.
    pub fn hostile_to(self, other: Faction) -> bool {
        if self == other {
            return false;
        }
        if self == Faction::Freebooters || other == Faction::Freebooters {
            return true;
        }
        // The player's Corsairs are hostile to the state powers, and vice versa.
        self != other
    }
}

/// Marker for anything that is a ship (as opposed to a projectile or pickup).
#[derive(Component, Default)]
pub struct Ship;

/// The ship the encounter revolves around — spawn director centres waves on it
/// and the encounter is lost if it dies. The client attaches this to the player;
/// the sim only needs to know "the protagonist", not "the player".
#[derive(Component, Default)]
pub struct Protagonist;

/// Facing angle in radians. `0.0` points along +X; positive is counter-clockwise.
/// This is the sim's source of truth for orientation; the movement system writes
/// it into `Transform.rotation` each step so any renderer sees a valid transform.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Heading(pub f32);

impl Heading {
    /// Unit vector the bow points along.
    pub fn forward(self) -> Vec2 {
        Vec2::from_angle(self.0)
    }
}

/// Linear velocity in world units per second.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Velocity(pub Vec2);

/// Handling characteristics of a hull. Tuned per ship class.
#[derive(Component, Clone, Copy, Debug)]
pub struct ShipStats {
    /// Forward acceleration at full throttle (units/s²).
    pub thrust: f32,
    /// Maximum turn rate at full helm (radians/s).
    pub turn_rate: f32,
    /// Speed cap (units/s). Black-Flag ships accelerate to a sail-set top speed.
    pub max_speed: f32,
    /// Fraction of speed bled off per second when coasting (0..1-ish).
    pub linear_drag: f32,
}

impl Default for ShipStats {
    fn default() -> Self {
        // A nimble corsair sloop.
        Self {
            thrust: 260.0,
            turn_rate: 2.2,
            max_speed: 340.0,
            linear_drag: 0.6,
        }
    }
}

/// Control intent for a ship, in the range `-1.0..=1.0` on each axis. The player
/// controller (client) or an AI controller (sim) writes this; the movement
/// system reads it. `throttle`: +forward / -reverse. `turn`: +port / -starboard.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Helm {
    pub throttle: f32,
    pub turn: f32,
}

/// Structural health. A ship is destroyed when `current <= 0`.
#[derive(Component, Clone, Copy, Debug)]
pub struct Hull {
    pub current: f32,
    pub max: f32,
}

impl Hull {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
}

/// Firing intent for this frame: which broadside(s) the controller wants to
/// discharge. Cleared/read by the weapons system. `port` is the ship's left
/// side (+90° from the bow), `starboard` the right (-90°).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct FireOrders {
    pub port: bool,
    pub starboard: bool,
}

/// A bank of side-mounted cannons — the core Black-Flag offensive weapon.
#[derive(Component, Clone, Copy, Debug)]
pub struct Broadside {
    /// Seconds between volleys.
    pub cooldown: f32,
    /// Remaining time until this bank can fire again.
    pub timer: f32,
    /// Damage per cannonball.
    pub damage: f32,
    /// Muzzle speed of each ball (units/s), added to the ship's velocity.
    pub muzzle_speed: f32,
    /// Number of guns per side (balls per volley, spread along the hull).
    pub guns: u32,
}

impl Default for Broadside {
    fn default() -> Self {
        Self {
            cooldown: 1.5,
            timer: 0.0,
            damage: 12.0,
            muzzle_speed: 520.0,
            guns: 3,
        }
    }
}

/// A cannonball in flight.
#[derive(Component, Clone, Copy, Debug)]
pub struct Projectile {
    pub damage: f32,
    /// Who fired it — so a ship never damages its own faction.
    pub faction: Faction,
    /// Collision radius for the hit check.
    pub radius: f32,
}

/// Time-to-live in seconds. When it reaches zero the entity is despawned.
#[derive(Component, Clone, Copy, Debug)]
pub struct Ttl(pub f32);

/// Collision radius of a ship, for projectile hit tests (broad-phase circle).
#[derive(Component, Clone, Copy, Debug)]
pub struct Collider {
    pub radius: f32,
}

impl Default for Collider {
    fn default() -> Self {
        Self { radius: 26.0 }
    }
}

/// A ship's brace state. While `active`, incoming damage is reduced — the
/// Black-Flag defensive move. The controller (player/AI) sets it each frame.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Brace {
    pub active: bool,
}

/// Marks a ship as crippled — hull driven low enough that it stops fighting and
/// drifts, boardable by the protagonist. Not destroyed: it can be looted (the
/// piracy payoff) or finished off with more fire.
#[derive(Component, Default)]
pub struct Disabled;

/// A fixed feature of the star system — the central star, a station, a planet.
/// Spatial/visual anchor now; a collision/interaction target later. `radius` is
/// its size for rendering and future hit tests.
#[derive(Component, Clone, Copy, Debug)]
pub struct Landmark {
    pub radius: f32,
}

/// Marks a ship as AI-controlled and carries its combat tuning. The AI system
/// writes this ship's [`Helm`] and [`FireOrders`]; a ship without it (the
/// player) is driven by the client instead. This keeps the sim ignorant of who
/// "the player" is — it only knows which ships steer themselves.
#[derive(Component, Clone, Copy, Debug)]
pub struct AiController {
    /// Inside this distance the ship stops closing and turns to present a beam.
    pub engage_range: f32,
    /// Half-width of the firing arc off each beam (radians). A shot is taken
    /// when the target sits within this arc of the port or starboard beam.
    pub fire_arc: f32,
    /// Below this fraction of max hull the ship breaks off and runs.
    pub flee_hull_frac: f32,
}

impl Default for AiController {
    fn default() -> Self {
        Self {
            engage_range: 300.0,
            fire_arc: 0.35, // ~20°
            flee_hull_frac: 0.25,
        }
    }
}
