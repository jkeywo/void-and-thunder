//! Global simulation tuning — the numbers that are rules rather than ship kit.
//!
//! Per-ship numbers (thrust, broadside damage, torpedo speed) live on the ship's
//! own components, so two ships can differ. What lives here is the handful of
//! values that apply to *every* ship at once: how much a brace cuts damage by,
//! how long a cannonball lives, how hard the system boundary pushes back, the
//! AI's control gains. They were `const`s; they are a [`SimTuning`] resource so a
//! designer can move them at runtime.
//!
//! The consts remain as the documented defaults — [`SimTuning::default`] is built
//! from them, so nothing changes unless something replaces the resource. The
//! client loads a RON file over the top; [`Harness`](crate::harness::Harness) and
//! every unit test just use the defaults and need no file at all.
//!
//! Pure functions take the value they need as an argument rather than reading the
//! resource, which is what keeps them testable without a `World`.

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ai::{
    STATION_THROTTLE, SURROUND_COUNT, SURROUND_RADIUS, TORPEDO_MIN_VOLLEY, TURN_GAIN, WARP_PRIME,
};
use crate::combat::{
    BRACE_DAMAGE_FACTOR, HULL_LENGTH, MUZZLE_STANDOFF, PROJECTILE_RADIUS, PROJECTILE_TTL,
};
use crate::components::ENGAGEMENT_RANGE;
use crate::piracy::{BOARD_DWELL, BOARD_RANGE, CRIPPLE_THRESHOLD};
use crate::ship::REVERSE_THROTTLE;
use crate::torpedo::TORPEDO_LAUNCH_INTERVAL;
use crate::world::BOUNDS_SPRING;

/// The AI's control gains and thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiTuning {
    /// Proportional gain turning a heading error (radians) into a helm command.
    pub turn_gain: f32,
    /// Throttle while jockeying for a broadside — mostly turning, holding station.
    pub station_throttle: f32,
    /// Enemies within this radius count toward being "surrounded".
    pub surround_radius: f32,
    /// This many nearby hostiles triggers a microwarp escape.
    pub surround_count: u32,
    /// Seconds the AI holds a microwarp to prime it before releasing (warping).
    pub warp_prime: f32,
    /// Torpedo locks the AI builds before releasing a volley.
    pub torpedo_min_volley: u32,
}

impl Default for AiTuning {
    fn default() -> Self {
        Self {
            turn_gain: TURN_GAIN,
            station_throttle: STATION_THROTTLE,
            surround_radius: SURROUND_RADIUS,
            surround_count: SURROUND_COUNT as u32,
            warp_prime: WARP_PRIME,
            torpedo_min_volley: TORPEDO_MIN_VOLLEY,
        }
    }
}

/// Simulation-wide tuning. Every field applies to the whole sim, not one ship.
///
/// `#[serde(default)]` throughout: a data file may omit any field and inherit the
/// default, so files stay hand-editable without knowing the whole schema and
/// adding a field never invalidates an existing file.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SimTuning {
    /// Fraction of damage a braced ship still takes.
    pub brace_damage_factor: f32,
    /// How long a cannonball lives before falling into the void (seconds). With
    /// a bank's muzzle speed this also sets the broadside's reach.
    pub projectile_ttl: f32,
    /// Cannonball collision radius.
    pub projectile_radius: f32,
    /// Length of hull the guns of a volley are spread along.
    pub hull_length: f32,
    /// How far ahead of the ship a volley's muzzles sit.
    pub muzzle_standoff: f32,
    /// Seconds between torpedoes leaving the tubes during a volley.
    pub torpedo_launch_interval: f32,
    /// Fraction of forward thrust available in reverse.
    pub reverse_throttle: f32,
    /// Spring strength pulling a ship back inside the system bounds.
    pub bounds_spring: f32,
    /// How far the ship's top-down kit reaches (microwarp jump, torpedo range).
    pub engagement_range: f32,
    /// Hull fraction at or below which an enemy is crippled and becomes boardable.
    pub cripple_threshold: f32,
    /// How close the protagonist must be to board a crippled ship.
    pub board_range: f32,
    /// Seconds the protagonist must hold position in range to claim a prize.
    pub board_dwell: f32,
    /// The AI's control gains.
    pub ai: AiTuning,
}

impl Default for SimTuning {
    fn default() -> Self {
        Self {
            brace_damage_factor: BRACE_DAMAGE_FACTOR,
            projectile_ttl: PROJECTILE_TTL,
            projectile_radius: PROJECTILE_RADIUS,
            hull_length: HULL_LENGTH,
            muzzle_standoff: MUZZLE_STANDOFF,
            torpedo_launch_interval: TORPEDO_LAUNCH_INTERVAL,
            reverse_throttle: REVERSE_THROTTLE,
            bounds_spring: BOUNDS_SPRING,
            engagement_range: ENGAGEMENT_RANGE,
            cripple_threshold: CRIPPLE_THRESHOLD,
            board_range: BOARD_RANGE,
            board_dwell: BOARD_DWELL,
            ai: AiTuning::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resource must reproduce the constants exactly, so mounting the sim
    /// without any data file behaves identically to the pre-tuning build.
    #[test]
    fn defaults_match_the_constants() {
        let t = SimTuning::default();
        assert_eq!(t.brace_damage_factor, 0.35);
        assert_eq!(t.projectile_ttl, 2.5);
        assert_eq!(t.projectile_radius, 5.0);
        assert_eq!(t.hull_length, 40.0);
        assert_eq!(t.muzzle_standoff, 22.0);
        assert_eq!(t.torpedo_launch_interval, 0.5);
        assert_eq!(t.reverse_throttle, 0.25);
        assert_eq!(t.bounds_spring, 3.0);
        assert_eq!(t.engagement_range, 675.0);
        assert_eq!(t.cripple_threshold, 0.25);
        assert_eq!(t.board_range, 95.0);
        assert_eq!(t.board_dwell, 3.0);
        assert_eq!(t.ai.turn_gain, 2.5);
        assert_eq!(t.ai.surround_count, 2);
        assert_eq!(t.ai.torpedo_min_volley, 3);
    }
}
