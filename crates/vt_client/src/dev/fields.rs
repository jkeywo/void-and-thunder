//! What the design panel needs to know about a field that its type cannot say.
//!
//! `bevy_reflect` gives the panel field *enumeration* and typed read/write for
//! free, so one walker handles every tuning struct and a field added later
//! appears without touching this file. What reflection cannot supply is the two
//! things a `Default` impl doesn't carry:
//!
//! * a sensible slider range — the type says `f32`, not "0 to 3000";
//! * whether a field is authored config or live state — `Broadside.port` holds a
//!   reload timer that must never be written by a class edit, and `TorpedoBay.
//!   loaded` is a magazine that would refill on every keystroke.
//!
//! Hence this table. The *mechanism* — what a descriptor is, how one is
//! looked up, and the heuristic fallback for a field nobody has described —
//! lives in `vellum-editor`; what lives here is the part only this game can
//! know: that a thrust runs to 3000 and that `Broadside.port` is a reload
//! timer nobody should be typing into.
//!
//! Anything missing from the table still works: it gets a text box, a
//! heuristic range, and is treated as config. That degradation is the point —
//! a new field being *badly ranged* is a small problem, a new field being
//! *invisible* is the problem this whole feature exists to avoid.
//!
//! Entries are keyed by **owning struct and field**, not by field name alone.
//! Bare names collide: `Broadside.damage` is authored config while
//! `EmpDefense.damage` is how much EMP a ship has soaked, and treating the
//! second as the first would wipe a ship's EMP load every time anything was
//! retuned.

use vellum_editor::SpecTable;
pub use vellum_editor::{FieldKind, FieldSpec};

/// Ranges chosen to put the shipped value somewhere useful on the slider —
/// roughly a third to half way — so there is room to push a number both ways.
static FIELDS: SpecTable = SpecTable::new(&[
    // ShipStats — how a hull handles.
    ("ShipStats", "thrust", FieldSpec::config(0.0, 3000.0)),
    ("ShipStats", "turn_rate", FieldSpec::config(0.0, 4.0)),
    ("ShipStats", "max_speed", FieldSpec::config(0.0, 400.0)),
    ("ShipStats", "linear_drag", FieldSpec::config(0.0, 20.0)),
    // Broadside.
    ("Broadside", "cooldown", FieldSpec::config(0.0, 20.0)),
    ("Broadside", "damage", FieldSpec::config(0.0, 100.0)),
    ("Broadside", "muzzle_speed", FieldSpec::config(0.0, 1000.0)),
    ("Broadside", "guns", FieldSpec::config(1.0, 12.0)),
    // Half-angle: pi is a full 360-degree arc.
    (
        "Broadside",
        "arc",
        FieldSpec::config(0.0, std::f32::consts::PI),
    ),
    ("Broadside", "charge_time", FieldSpec::config(0.0, 3.0)),
    // Live per-bank reload/telegraph state.
    ("Broadside", "port", FieldSpec::live()),
    ("Broadside", "starboard", FieldSpec::live()),
    // TorpedoBay.
    ("TorpedoBay", "tubes_max", FieldSpec::config(1.0, 6.0)),
    ("TorpedoBay", "loaded", FieldSpec::live()),
    (
        "TorpedoBay",
        "reload_per_tube",
        FieldSpec::config(0.0, 10.0),
    ),
    ("TorpedoBay", "lock_interval", FieldSpec::config(0.0, 3.0)),
    ("TorpedoBay", "lock_radius", FieldSpec::config(0.0, 400.0)),
    ("TorpedoBay", "range", FieldSpec::config(0.0, 2000.0)),
    ("TorpedoBay", "turn_rate", FieldSpec::config(0.0, 10.0)),
    ("TorpedoBay", "speed", FieldSpec::config(0.0, 1000.0)),
    ("TorpedoBay", "damage", FieldSpec::config(0.0, 100.0)),
    // EmpWeapon.
    ("EmpWeapon", "swivel_rate", FieldSpec::config(0.0, 3.0)),
    ("EmpWeapon", "aim", FieldSpec::live()),
    ("EmpWeapon", "timer", FieldSpec::live()),
    ("EmpWeapon", "bolt_speed", FieldSpec::config(0.0, 1000.0)),
    ("EmpWeapon", "bolt_damage_frac", FieldSpec::config(0.0, 1.0)),
    ("EmpWeapon", "range", FieldSpec::config(0.0, 2000.0)),
    // EmpDefense. Note `damage` here is EMP *soaked*, not damage dealt — a
    // live value, and the reason this table is keyed by owner and not by name.
    ("EmpDefense", "damage", FieldSpec::live()),
    ("EmpDefense", "resist", FieldSpec::config(0.0, 400.0)),
    (
        "EmpDefense",
        "recovery_per_sec",
        FieldSpec::config(0.0, 60.0),
    ),
    // Drives.
    ("BoostDrive", "multiplier", FieldSpec::config(1.0, 4.0)),
    ("BoostDrive", "battery", FieldSpec::live()),
    ("BoostDrive", "battery_max", FieldSpec::config(0.0, 20.0)),
    ("BoostDrive", "drain_per_sec", FieldSpec::config(0.0, 5.0)),
    (
        "BoostDrive",
        "recharge_per_sec",
        FieldSpec::config(0.0, 5.0),
    ),
    ("MicrowarpDrive", "was_holding", FieldSpec::live()),
    ("MicrowarpDrive", "timer", FieldSpec::live()),
    ("MicrowarpDrive", "cooldown", FieldSpec::config(0.0, 60.0)),
    ("MicrowarpDrive", "range", FieldSpec::config(0.0, 2000.0)),
    // AiController.
    (
        "AiController",
        "engage_range",
        FieldSpec::config(0.0, 1500.0),
    ),
    (
        "AiController",
        "fire_arc",
        FieldSpec::config(0.0, std::f32::consts::PI),
    ),
    (
        "AiController",
        "flee_hull_frac",
        FieldSpec::config(0.0, 1.0),
    ),
    ("AiController", "warp_prime", FieldSpec::live()),
    // SimTuning — whole-sim rules.
    (
        "SimTuning",
        "brace_damage_factor",
        FieldSpec::config(0.0, 1.0),
    ),
    ("SimTuning", "projectile_ttl", FieldSpec::config(0.0, 10.0)),
    (
        "SimTuning",
        "projectile_radius",
        FieldSpec::config(0.0, 40.0),
    ),
    ("SimTuning", "hull_length", FieldSpec::config(0.0, 200.0)),
    (
        "SimTuning",
        "muzzle_standoff",
        FieldSpec::config(0.0, 100.0),
    ),
    (
        "SimTuning",
        "torpedo_launch_interval",
        FieldSpec::config(0.0, 3.0),
    ),
    ("SimTuning", "reverse_throttle", FieldSpec::config(0.0, 1.0)),
    ("SimTuning", "bounds_spring", FieldSpec::config(0.0, 20.0)),
    (
        "SimTuning",
        "engagement_range",
        FieldSpec::config(0.0, 2000.0),
    ),
    (
        "SimTuning",
        "cripple_threshold",
        FieldSpec::config(0.0, 1.0),
    ),
    ("SimTuning", "board_range", FieldSpec::config(0.0, 400.0)),
    ("SimTuning", "board_dwell", FieldSpec::config(0.0, 15.0)),
    // AiTuning.
    ("AiTuning", "turn_gain", FieldSpec::config(0.0, 10.0)),
    ("AiTuning", "station_throttle", FieldSpec::config(0.0, 1.0)),
    (
        "AiTuning",
        "surround_radius",
        FieldSpec::config(0.0, 1500.0),
    ),
    ("AiTuning", "surround_count", FieldSpec::config(1.0, 8.0)),
    (
        "AiTuning",
        "torpedo_min_volley",
        FieldSpec::config(1.0, 6.0),
    ),
    // Hull / collider.
    ("ShipClass", "hull", FieldSpec::config(1.0, 500.0)),
    ("Collider", "radius", FieldSpec::config(1.0, 100.0)),
]);

/// This game's descriptor table.
pub fn specs() -> &'static SpecTable {
    &FIELDS
}

/// The spec for a field, falling back to the shared heuristic when the table
/// is silent.
pub fn spec_for(owner: &str, field: &str, current: f32) -> FieldSpec {
    FIELDS.spec_for(owner, field, current)
}

pub use vellum_editor::owner_of;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_fields_are_marked_live() {
        for (owner, field) in [
            ("Broadside", "port"),
            ("Broadside", "starboard"),
            ("TorpedoBay", "loaded"),
            ("BoostDrive", "battery"),
            ("AiController", "warp_prime"),
            ("EmpDefense", "damage"),
        ] {
            assert_eq!(
                spec_for(owner, field, 0.0).kind,
                FieldKind::Live,
                "{owner}.{field} is live state and must never be written by a class edit"
            );
        }
    }

    /// The reason this table is keyed by owner: the same field name means
    /// opposite things on two structs, and conflating them wiped a ship's EMP
    /// load every time anything else was retuned.
    #[test]
    fn the_same_field_name_can_mean_different_things() {
        assert_eq!(spec_for("Broadside", "damage", 0.0).kind, FieldKind::Config);
        assert_eq!(spec_for("EmpDefense", "damage", 0.0).kind, FieldKind::Live);
    }

    /// The property that makes this table safe to leave incomplete.
    #[test]
    fn an_unknown_field_still_gets_a_usable_range() {
        let spec = spec_for("ShipStats", "some_field_added_next_week", 40.0);
        assert_eq!(spec.kind, FieldKind::Config);
        assert!(
            spec.min <= 40.0 && spec.max >= 40.0,
            "range must contain it"
        );
        assert!(spec.max > spec.min, "a zero-width slider is unusable");
    }

    #[test]
    fn a_zero_valued_unknown_field_gets_a_non_empty_range() {
        let spec = spec_for("SomeStruct", "brand_new_field", 0.0);
        assert!(spec.max > spec.min);
    }

    /// The shipped values should sit somewhere usable on their sliders, not
    /// pinned at an end where the number can only move one way.
    #[test]
    fn shipped_values_sit_inside_their_ranges() {
        let stats = vt_sim::prelude::ShipStats::default();
        for (name, value) in [
            ("thrust", stats.thrust),
            ("turn_rate", stats.turn_rate),
            ("max_speed", stats.max_speed),
            ("linear_drag", stats.linear_drag),
        ] {
            let spec = spec_for("ShipStats", name, value);
            assert!(
                value > spec.min && value < spec.max,
                "{name} = {value} is not strictly inside {}..{}",
                spec.min,
                spec.max
            );
        }
    }
}
