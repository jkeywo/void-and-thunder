//! Game feel, as data: bullet-time, screen shake, engine trails, the camera.
//!
//! These are the numbers that decide how the game *feels* rather than what it
//! does. Nothing here changes a rule — you cannot win a fight by tuning screen
//! shake — but they are exactly the values worth iterating on with the game
//! running, which is why they sit beside the sim's tuning rather than staying
//! compiled in.
//!
//! Deliberately in `data/`, not `dev/`: this is loaded and applied in every
//! build, including one with no design panel. Feature-gating it would mean a
//! release ran on the compiled-in defaults while the tuned RON sat unread.
//!
//! As with [`SimTuning`](vt_sim::prelude::SimTuning), the consts stay as the
//! documented defaults and `Default` is built from them, so a missing file
//! degrades to exactly the shipped behaviour.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Slow-motion and freeze-frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct TimeFeel {
    /// Fraction of normal time while aiming (bullet-time).
    pub aim_timescale: f32,
    /// Fraction of normal time during a hitstop — near enough a freeze-frame.
    pub hitstop_timescale: f32,
    /// Seconds of aim time the battery holds, spent in *real* seconds so the
    /// window is the same length however slow the world is running.
    pub battery_max: f32,
    pub battery_drain_per_sec: f32,
    pub battery_recharge_per_sec: f32,
}

impl Default for TimeFeel {
    fn default() -> Self {
        Self {
            aim_timescale: 0.10,
            hitstop_timescale: 0.05,
            battery_max: 5.0,
            battery_drain_per_sec: 1.0,
            battery_recharge_per_sec: 1.0,
        }
    }
}

/// Impact: screen shake, camera kick, freeze-frames and debris.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct ImpactFeel {
    /// Beyond this distance from the player, an event shakes nothing.
    pub shake_range: f32,
    /// Trauma from being hit, plus a per-point-of-damage bonus.
    pub own_hit_trauma: f32,
    pub own_hit_trauma_per_damage: f32,
    /// How hard the camera is kicked when the player is hit.
    pub own_hit_kick: f32,
    /// Trauma from a hit landing on someone else nearby.
    pub nearby_hit_trauma: f32,
    /// Trauma from a death — your own, or a nearby ship's.
    pub own_death_trauma: f32,
    pub nearby_death_trauma: f32,
    /// Freeze-frame lengths, in real seconds.
    pub kill_hitstop: f32,
    pub own_hit_hitstop: f32,
    /// Debris thrown by an explosion.
    pub debris_count: u32,
    pub debris_speed_min: f32,
    pub debris_speed_max: f32,
    pub debris_life: f32,
    /// Sparks thrown back along a blow's travel when a hull is struck. Short and
    /// fast: this is the cue for *where the shot came from*, so it wants to be
    /// gone before the eye starts reading it as an explosion.
    pub spark_count: u32,
    /// Width of the spatter cone, in radians.
    pub spark_cone: f32,
    pub spark_speed_min: f32,
    pub spark_speed_max: f32,
    pub spark_life: f32,
    /// The wreck left where a ship died: how long it burns out over, how fast it
    /// tumbles (rad/s, each axis), and what fraction of the ship's way it keeps.
    pub wreck_life: f32,
    pub wreck_spin: f32,
    pub wreck_drift: f32,
    /// Gamepad rumble. `rumble_scale` turns the shake trauma of an event into a
    /// motor strength, so the pad and the camera stay in step by construction.
    pub rumble_scale: f32,
    pub rumble_hit_secs: f32,
    pub rumble_death_secs: f32,
    /// How long a muzzle flash lingers.
    pub flash_time: f32,
}

impl Default for ImpactFeel {
    fn default() -> Self {
        Self {
            shake_range: 900.0,
            own_hit_trauma: 0.18,
            own_hit_trauma_per_damage: 0.010,
            own_hit_kick: 22.0,
            nearby_hit_trauma: 0.10,
            own_death_trauma: 0.9,
            nearby_death_trauma: 0.45,
            kill_hitstop: 0.06,
            own_hit_hitstop: 0.04,
            debris_count: 8,
            debris_speed_min: 90.0,
            debris_speed_max: 220.0,
            debris_life: 0.35,
            spark_count: 5,
            spark_cone: 1.2,
            spark_speed_min: 70.0,
            spark_speed_max: 180.0,
            spark_life: 0.18,
            wreck_life: 1.1,
            wreck_spin: 3.0,
            wreck_drift: 0.55,
            rumble_scale: 1.6,
            rumble_hit_secs: 0.16,
            rumble_death_secs: 0.55,
            flash_time: 0.08,
        }
    }
}

/// Where the camera sits and how quickly it gets there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct CameraFeel {
    /// Resting distance behind, and height above, the ship.
    pub distance: f32,
    pub height: f32,
    /// How fast the camera's yaw chases the ship's heading.
    pub yaw_lerp: f32,
    /// Resting pitch, and the limits free-look may push it to.
    pub pitch_base: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    /// Pitch and distance multiplier while aiming a broadside — lower and
    /// closer, so the arc fills the screen.
    pub aim_pitch: f32,
    pub aim_dist: f32,
    /// Pitch and distance for the top-down modes (torpedoes, microwarp).
    pub topdown_pitch: f32,
    pub topdown_dist: f32,
    /// How fast the camera eases into an aim mode, and how fast distance follows.
    pub aim_lerp: f32,
    pub dist_lerp: f32,
    /// Free-look sensitivity and how far it may swing.
    pub look_yaw_rate: f32,
    pub look_pitch_rate: f32,
    /// Seconds of no input before the camera recentres, and how fast it does.
    pub recenter_delay: f32,
    pub recenter_lerp: f32,
    /// How far ahead of the ship the camera looks, and the cap on that lead.
    pub lead_secs: f32,
    pub lead_max: f32,
    /// Field of view at rest, and the extra it gains at full boost.
    pub base_fov: f32,
    pub boost_fov_gain: f32,
    pub fov_lerp: f32,
    /// How fast the menu camera orbits the parked ship.
    pub menu_orbit_rate: f32,
    /// How quickly an impact kick decays, and how far one may ever shove the eye.
    pub kick_decay: f32,
    pub kick_max: f32,
    /// Screen shake. `trauma_decay` is how fast a shock bleeds off (per second),
    /// `shake_magnitude` how far the eye is thrown at full trauma, and
    /// `shake_freq` how many times a second the noise picks a new target — the
    /// rig interpolates between them, so this is the difference between a shake
    /// that reads as an impact and one that reads as television static.
    pub trauma_decay: f32,
    pub shake_magnitude: f32,
    pub shake_freq: f32,
    /// How tightly the camera's focus point chases the ship.
    pub focus_lerp: f32,
    /// Seconds of no look input before a *pad* recentres. Much shorter than
    /// `recenter_delay`: a stick springs back on its own, so letting go is a
    /// deliberate "put the view back", where a mouse holds where it was left.
    pub recenter_delay_pad: f32,
}

impl Default for CameraFeel {
    fn default() -> Self {
        Self {
            distance: 430.0,
            height: 170.0,
            yaw_lerp: 2.5,
            pitch_base: 0.85,
            pitch_min: 0.35,
            pitch_max: 1.2,
            aim_pitch: 0.28,
            aim_dist: 0.62,
            topdown_pitch: 1.5,
            topdown_dist: 1125.0,
            aim_lerp: 9.0,
            dist_lerp: 6.0,
            look_yaw_rate: 2.4,
            look_pitch_rate: 1.8,
            recenter_delay: 3.0,
            recenter_lerp: 1.6,
            lead_secs: 0.25,
            lead_max: 120.0,
            base_fov: std::f32::consts::FRAC_PI_4,
            boost_fov_gain: 0.14,
            fov_lerp: 5.0,
            menu_orbit_rate: 0.15,
            kick_decay: 9.0,
            kick_max: 60.0,
            trauma_decay: 1.4,
            shake_magnitude: 26.0,
            shake_freq: 34.0,
            focus_lerp: 10.0,
            recenter_delay_pad: 0.1,
        }
    }
}

/// The status rings drawn on the plane beneath each ship.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct RingFeel {
    /// Opacity of the whole ring layer, `0..1`.
    ///
    /// Applied to the entire element rather than to each band, so the ring
    /// fades as one object and the relative weight the designer set between
    /// hull, shields and guns is preserved at every setting. Per-band alpha
    /// would let the bands drift out of balance as it was turned down.
    ///
    /// It is a whole-layer knob because the ring sits *over the ship it
    /// describes*: at full strength a charged shield band is a solid disc
    /// across the hull, and how much of the ship you are willing to lose to
    /// the readout is a taste call, not a constant.
    pub opacity: f32,
    /// How far below the ship's own plane the ring is drawn, in world units.
    ///
    /// The ring is a mark on the deck, so it has to sit clear *under* the hull
    /// rather than cutting through it. Bounded at both ends: too little and the
    /// model intersects the bands, too much and it sinks past the reference grid
    /// at z -9 and gets crossed by grid lines.
    pub drop: f32,
}

impl Default for RingFeel {
    fn default() -> Self {
        Self {
            opacity: 0.5,
            drop: 8.0,
        }
    }
}

/// How the controls read: pointer sensitivity and stick shaping.
///
/// These were compiled in, which meant the two numbers a player is most likely
/// to want changed — mouse sensitivity and stick deadzone — were the only ones
/// in the game that needed a rebuild, while trail colours were hot-reloadable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct ControlFeel {
    /// How much of the broadside arc a pixel of mouse motion sweeps.
    pub mouse_aim_sens: f32,
    /// How fast the pad drives the aim cursor, and how far from the ship it may
    /// be pushed.
    pub aim_cursor_rate: f32,
    pub aim_cursor_max: f32,
    /// Stick shaping: everything under `deadzone` reads as centred, everything
    /// over `saturation` reads as fully deflected, and the span between is
    /// rescaled across the whole range.
    pub deadzone: f32,
    pub saturation: f32,
}

impl Default for ControlFeel {
    fn default() -> Self {
        Self {
            mouse_aim_sens: 0.0032,
            aim_cursor_rate: 780.0,
            aim_cursor_max: 1300.0,
            deadzone: 0.05,
            saturation: 0.90,
        }
    }
}

/// One engine or torpedo ribbon's look.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct RibbonFeel {
    /// Colour at the hot (near) end and the cool (far) end, as linear RGB.
    pub hot: [f32; 3],
    pub cool: [f32; 3],
    pub width: f32,
    /// Seconds a crumb of trail lives.
    pub lifetime: f32,
    /// Crumbs the ribbon may hold, and how far the source must move to drop one.
    pub max_crumbs: u32,
    pub min_step: f32,
}

/// Engine and torpedo trails.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct TrailFeel {
    pub engine: RibbonFeel,
    pub torpedo: RibbonFeel,
    /// Where the engine ribbons attach: aft of centre, and out to each nacelle.
    pub stern_x: f32,
    pub nacelle_y: f32,
    /// Below this throttle no trail is emitted at all.
    pub throttle_deadzone: f32,
    /// Width multiplier while boosting.
    pub boost_width: f32,
    /// How far below the plane a torpedo's tail sits.
    pub torpedo_tail_z: f32,
    /// How much a ribbon narrows along its length.
    pub width_falloff: f32,
}

impl Default for RibbonFeel {
    fn default() -> Self {
        Self {
            hot: [0.78, 0.93, 1.0],
            cool: [0.12, 0.32, 0.9],
            width: 5.0,
            lifetime: 0.9,
            max_crumbs: 64,
            min_step: 5.0,
        }
    }
}

impl Default for TrailFeel {
    fn default() -> Self {
        Self {
            engine: RibbonFeel::default(),
            torpedo: RibbonFeel {
                hot: [1.0, 0.86, 0.55],
                cool: [0.9, 0.28, 0.05],
                width: 3.5,
                lifetime: 0.45,
                max_crumbs: 48,
                min_step: 4.0,
            },
            stern_x: -20.0,
            nacelle_y: 6.0,
            throttle_deadzone: 0.05,
            boost_width: 1.7,
            torpedo_tail_z: -13.0,
            width_falloff: 0.55,
        }
    }
}

/// Everything about how the game feels, in one resource.
// `TypePath` is not derived here: `Reflect` already provides it, and asking for
// both is a conflicting-impl error.
#[derive(
    Asset, Resource, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect,
)]
#[serde(default)]
pub struct FeelTuning {
    pub time: TimeFeel,
    pub impact: ImpactFeel,
    pub camera: CameraFeel,
    pub controls: ControlFeel,
    pub rings: RingFeel,
    pub trails: TrailFeel,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped file must reproduce the compiled-in feel exactly, so tuning
    /// nothing changes nothing.
    #[test]
    fn shipped_feel_matches_the_defaults() {
        let text = include_str!("../../assets/data/feel.tuning.ron");
        let parsed: FeelTuning = ron::from_str(text).expect("feel.tuning.ron should parse");
        assert_eq!(
            parsed,
            FeelTuning::default(),
            "assets/data/feel.tuning.ron has drifted from FeelTuning::default()"
        );
    }

    /// A partial file must inherit the rest rather than zeroing it — this is
    /// what makes the files safe to trim down to just the overrides.
    #[test]
    fn an_omitted_section_inherits_its_defaults() {
        let parsed: FeelTuning =
            ron::from_str("(time: (aim_timescale: 0.5))").expect("a partial file parses");
        assert_eq!(parsed.time.aim_timescale, 0.5, "the override lands");
        assert_eq!(
            parsed.time.hitstop_timescale,
            TimeFeel::default().hitstop_timescale,
            "an omitted sibling keeps its default"
        );
        assert_eq!(
            parsed.camera,
            CameraFeel::default(),
            "an omitted section keeps its defaults"
        );
    }
}
