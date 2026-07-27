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
use bevy_reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::ai::{
    STATION_THROTTLE, SURROUND_COUNT, SURROUND_RADIUS, TORPEDO_MIN_VOLLEY, TURN_EASE, TURN_GAIN,
    WARP_PRIME,
};
use crate::collide::{
    RAM_BOOST_BONUS, RAM_DAMAGE_PER_SPEED, RAM_DAMAGE_THRESHOLD, RAM_RESTITUTION, RAM_SEPARATION,
};
use crate::combat::{
    BRACE_DAMAGE_FACTOR, HULL_LENGTH, MUZZLE_STANDOFF, PROJECTILE_RADIUS, PROJECTILE_TTL,
};
use crate::components::ENGAGEMENT_RANGE;
use crate::piracy::{BOARD_DWELL, BOARD_RANGE, BOARD_REPAIR_FRAC, CRIPPLE_THRESHOLD};
use crate::ship::REVERSE_THROTTLE;
use crate::torpedo::TORPEDO_LAUNCH_INTERVAL;
use crate::world::BOUNDS_SPRING;

/// The AI's control gains and thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct AiTuning {
    /// Proportional gain turning a heading error (radians) into a helm command.
    pub turn_gain: f32,
    /// Throttle while jockeying for a broadside — mostly turning, holding station.
    pub station_throttle: f32,
    /// How much of its throttle the AI spills to buy turn rate, at a full 180°
    /// of heading error. `0.0` means it never slows to turn (and, at full sail,
    /// orbits its target forever without ever closing).
    pub turn_ease: f32,
    /// Enemies within this radius count toward being "surrounded".
    pub surround_radius: f32,
    /// This many nearby hostiles triggers a microwarp escape.
    pub surround_count: u32,
    /// Seconds the AI holds a microwarp to prime it before releasing (warping).
    pub warp_prime: f32,
    /// Torpedo locks the AI builds before releasing a volley.
    pub torpedo_min_volley: u32,
    /// What the utility pilot values.
    pub pilot: PilotTuning,
}

impl Default for AiTuning {
    fn default() -> Self {
        Self {
            turn_gain: TURN_GAIN,
            station_throttle: STATION_THROTTLE,
            turn_ease: TURN_EASE,
            surround_radius: SURROUND_RADIUS,
            surround_count: SURROUND_COUNT as u32,
            warp_prime: WARP_PRIME,
            torpedo_min_volley: TORPEDO_MIN_VOLLEY,
            pilot: PilotTuning::default(),
        }
    }
}

/// What the utility pilot ([`pilot`](crate::pilot)) values — the weights and
/// shape numbers its cost functions are built from.
///
/// Unlike the rest of the tuning these have no constant of their own: they are
/// not a rule of the world that some system also reads, they *are* the brain's
/// judgement, and the only sensible way to arrive at them is to fly the ship and
/// move them. The `w_*` weights are the headline dials — each scales one action's
/// whole score, so raising `w_ram` makes a bolder pilot and nothing else. The
/// rest shape a single curve and are documented where they bite.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct PilotTuning {
    /// Weight on working the beam — the yardstick the others are read against.
    pub w_broadside: f32,
    /// Weight on standing off with torpedoes.
    pub w_torpedo: f32,
    /// Weight on turning the bow to work the EMP. Deliberately well under
    /// `w_broadside`: the emitter has twice the reach of the guns, so a pilot
    /// that valued it evenly would fight every engagement bow-on and never fire
    /// a broadside.
    pub w_emp: f32,
    /// Weight on ramming.
    pub w_ram: f32,
    /// Weight on breaking off to take a prize.
    pub w_board: f32,
    /// Weight on spending the microwarp.
    pub w_microwarp: f32,
    /// Weight on running. Above 1.0 so a decision to live can outbid a perfectly
    /// good broadside.
    pub w_disengage: f32,
    /// Bonus the action already being flown adds to its own score, so a pilot
    /// commits to a manoeuvre instead of twitching between two that happen to be
    /// scoring within a whisker of each other.
    pub commit_bonus: f32,
    /// How interesting the beam still is with both banks reloading — above zero,
    /// because a ship between volleys still wants to be presenting a side.
    pub reloading_interest: f32,
    /// Floor on the torpedo score as the magazine empties: at zero stores the
    /// bay is worth this fraction of a full one, so the last torpedoes are held
    /// for something worth spending them on.
    pub scarcity_floor: f32,
    /// How interesting the EMP is while the ship has guns or tubes ready. The
    /// gap between this and 1.0 is the whole "weapon of opportunity" idea.
    pub emp_busy_interest: f32,
    /// Throttle held while jockeying to keep a target under the emitter.
    pub emp_throttle: f32,
    /// Hull fraction below which the pilot will not trade rams — the damage is
    /// symmetric, so a wounded ship is losing that exchange.
    pub ram_hull_floor: f32,
    /// How much a prize is worth to a pilot in no particular need of one.
    pub board_base: f32,
    /// Extra pull toward a prize per point of missing hull (boarding repairs).
    pub board_repair_pull: f32,
    /// Extra pull toward a prize per point of missing stores (boarding resupplies).
    pub board_resupply_pull: f32,
    /// Threat level at which boarding is off the table. A boarding is seconds
    /// sitting still, which is not something to do with company.
    pub board_safe_threat: f32,
    /// Threat level that reads as "surrounded" for the escape jump.
    pub warp_escape_threat: f32,
    /// How interesting it is to spend the drive merely crossing the field, as a
    /// fraction of how interesting it is to escape with it.
    pub warp_reposition_interest: f32,
    /// Where the torpedo stance holds station, as a fraction of the bay's reach.
    pub torpedo_standoff: f32,
    /// How close a hostile must be, as a fraction of gun reach, before bracing
    /// between volleys is worth it.
    pub brace_range_frac: f32,
    /// What a point of shield is worth against a point of hull when the pilot
    /// asks how much punishment it has left. Under 1.0 because shields are
    /// directional and regenerate: a full bank is real protection, but only on
    /// the side it faces, so it should not read as flatly equivalent to plate.
    pub shield_worth: f32,
    /// How strongly the shields pull on the choice of facing. This is what turns
    /// "my bow bank is flat and still being shot at" into "show them the stern";
    /// at zero the pilot ignores its arcs entirely.
    pub shield_bias: f32,
    /// Floor under the facing multiplier, so presenting the worse side is a
    /// penalty rather than a veto — sometimes ramming is still the answer.
    pub presentation_floor: f32,
    /// Seconds a thumb takes to cross from one face button to another, during
    /// which neither is pressed. The pad has four of them and one thumb, so this
    /// is the price of changing your mind about which — and the reason the pilot
    /// cannot boost, brace and EMP in the same breath.
    pub thumb_travel: f32,
}

impl Default for PilotTuning {
    fn default() -> Self {
        Self {
            w_broadside: 1.0,
            w_torpedo: 0.95,
            w_emp: 0.7,
            w_ram: 0.9,
            w_board: 1.1,
            w_microwarp: 1.4,
            w_disengage: 1.8,
            commit_bonus: 0.15,
            reloading_interest: 0.55,
            scarcity_floor: 0.35,
            emp_busy_interest: 0.35,
            emp_throttle: 0.35,
            ram_hull_floor: 0.5,
            board_base: 0.35,
            board_repair_pull: 0.45,
            board_resupply_pull: 0.4,
            board_safe_threat: 1.0,
            warp_escape_threat: 2.0,
            warp_reposition_interest: 0.6,
            torpedo_standoff: 0.8,
            brace_range_frac: 0.6,
            shield_worth: 0.7,
            shield_bias: 0.35,
            presentation_floor: 0.4,
            thumb_travel: 0.2,
        }
    }
}

/// Simulation-wide tuning. Every field applies to the whole sim, not one ship.
///
/// `#[serde(default)]` throughout: a data file may omit any field and inherit the
/// default, so files stay hand-editable without knowing the whole schema and
/// adding a field never invalidates an existing file.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
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
    /// Fraction of maximum hull a boarding repairs.
    pub board_repair_frac: f32,
    /// Fraction of closing speed given back as bounce when hulls collide.
    pub ram_restitution: f32,
    /// Closing speed below which a hull-to-hull contact costs nothing.
    pub ram_damage_threshold: f32,
    /// Hull damage per unit of closing speed above the ram threshold.
    pub ram_damage_per_speed: f32,
    /// Fraction of a hull overlap corrected per step.
    pub ram_separation: f32,
    /// Extra ram damage for driving a boosted bow squarely into someone.
    pub ram_boost_bonus: f32,
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
            board_repair_frac: BOARD_REPAIR_FRAC,
            ram_restitution: RAM_RESTITUTION,
            ram_damage_threshold: RAM_DAMAGE_THRESHOLD,
            ram_damage_per_speed: RAM_DAMAGE_PER_SPEED,
            ram_separation: RAM_SEPARATION,
            ram_boost_bonus: RAM_BOOST_BONUS,
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
        assert_eq!(t.engagement_range, 506.0);
        assert_eq!(t.cripple_threshold, 0.25);
        assert_eq!(t.board_range, 95.0);
        assert_eq!(t.board_dwell, 3.0);
        assert_eq!(t.board_repair_frac, 0.10);
        assert_eq!(t.ram_restitution, 0.35);
        assert_eq!(t.ram_damage_threshold, 45.0);
        assert_eq!(t.ai.turn_gain, 2.5);
        assert_eq!(t.ai.surround_count, 2);
        assert_eq!(t.ai.torpedo_min_volley, 3);
    }
}
