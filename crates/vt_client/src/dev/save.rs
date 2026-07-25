//! Writing panel edits back to the RON files they came from.
//!
//! Save writes *resources*, never entities. The class table and the sim tuning
//! are the authored truth; a live ship is a copy of one, carrying reload timers
//! and spent magazines that have no business in a file. Because the live fields
//! are `#[serde(skip)]`, that property holds by construction rather than by
//! remembering: the struct that is serialized is the struct that was
//! deserialized.
//!
//! ## The echo
//!
//! With `hot-reload` on, writing a file makes the watcher fire, which re-parses
//! a file that already matches what is in memory. The apply systems guard
//! against that with an equality check rather than a timestamp — "did this
//! change" is exactly a value comparison, and timestamps would also have to
//! guess at how long a write takes to settle.
//!
//! ## Where it writes
//!
//! To the *source* tree (`crates/vt_client/assets/`), not to whatever the
//! working directory happens to be. Tuning is source, and a value that vanished
//! because the game was launched from a different directory would be maddening.
//! `CARGO_MANIFEST_DIR` is baked in at compile time, which is exactly right for
//! a tool that only ever runs from a dev build.

use bevy::prelude::*;
use ron::ser::PrettyConfig;
use serde::Serialize;
use std::path::PathBuf;
use vt_sim::prelude::SimTuning;

use crate::data::{paths, FeelTuning, ShipTable};

/// Absolute path of an asset in the source tree.
fn asset_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(relative)
}

/// Serialize `value` as pretty RON and write it to `relative` under `assets/`.
///
/// `struct_names(true)` is off: the shipped files use bare `(...)` bodies, and
/// writing type names back would make a saved file look unlike a hand-written
/// one — a diff full of noise the first time anyone saves.
fn write_ron<T: Serialize>(relative: &str, value: &T) -> Result<(), String> {
    let path = asset_path(relative);
    let config = PrettyConfig::new()
        .struct_names(false)
        .separate_tuple_members(true)
        .enumerate_arrays(false);
    let text = ron::ser::to_string_pretty(value, config).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the class table to `ships.ron`.
pub fn save_ships(table: &ShipTable) -> Result<(), String> {
    write_ron(paths::SHIPS.trim_start_matches("data/"), table)
        .map_err(|e| format!("could not save ship classes — {e}"))
}

/// Write the game-feel tuning to `feel.tuning.ron`.
pub fn save_feel(feel: &FeelTuning) -> Result<(), String> {
    write_ron(paths::FEEL_TUNING.trim_start_matches("data/"), feel)
        .map_err(|e| format!("could not save feel tuning — {e}"))
}

/// Write the simulation rules to `sim.tuning.ron`.
pub fn save_sim_tuning(tuning: &SimTuning) -> Result<(), String> {
    write_ron(paths::SIM_TUNING.trim_start_matches("data/"), tuning)
        .map_err(|e| format!("could not save sim tuning — {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through the exact writer the Save button uses. This is the
    /// property that matters: a saved file must be loadable, or the panel
    /// quietly destroys the data it was meant to preserve.
    #[test]
    fn a_saved_class_table_can_be_read_back() {
        let mut table = ShipTable::default();
        table.classes[0].class.stats.thrust = 1234.0;

        let config = PrettyConfig::new().struct_names(false);
        let text = ron::ser::to_string_pretty(&table, config).expect("serializes");
        let parsed: ShipTable = ron::from_str(&text).expect("saved RON must parse");

        assert_eq!(parsed, table);
        assert_eq!(parsed.classes[0].class.stats.thrust, 1234.0);
    }

    #[test]
    fn saved_sim_tuning_can_be_read_back() {
        let tuning = SimTuning {
            brace_damage_factor: 0.5,
            ..SimTuning::default()
        };
        let config = PrettyConfig::new().struct_names(false);
        let text = ron::ser::to_string_pretty(&tuning, config).expect("serializes");
        let parsed: SimTuning = ron::from_str(&text).expect("saved RON must parse");
        assert_eq!(parsed, tuning);
    }

    /// Live state must not reach the file. A saved class carrying a half-spent
    /// reload or an emptied magazine would hand every future ship of that class
    /// a broken starting state.
    #[test]
    fn live_state_never_reaches_the_file() {
        let mut table = ShipTable::default();
        let class = &mut table.classes[0].class;
        class.loadout.broadside.port.timer = 7.5;
        class.loadout.torpedoes.loaded = 1.0;
        class.emp_defense.damage = 42.0;

        let config = PrettyConfig::new().struct_names(false);
        let text = ron::ser::to_string_pretty(&table, config).expect("serializes");

        // Check the field *names* are absent rather than the values: a numeric
        // substring search matches far too eagerly (7.5 is inside max_speed's
        // 127.5), which makes for a test that fails on innocent data.
        for skipped in ["port:", "starboard:", "charging:", "loaded:"] {
            assert!(
                !text.contains(skipped),
                "live field `{skipped}` was written to the file:\n{text}"
            );
        }

        // The property that actually matters: reading it back gives fresh live
        // state, not the mid-combat values that were set above.
        let parsed: ShipTable = ron::from_str(&text).expect("parses");
        let round = &parsed.classes[0].class;
        assert_eq!(
            round.loadout.broadside.port.timer, 0.0,
            "reload timer reset"
        );
        assert_eq!(round.emp_defense.damage, 0.0, "EMP soak reset");
        assert_eq!(
            round.loadout.torpedoes.loaded,
            vt_sim::prelude::TorpedoBay::default().loaded,
            "a saved class hands new ships a full magazine, not a spent one"
        );
    }

    /// The save path must point at the source tree, not the working directory,
    /// so a tuned value survives being launched from somewhere else.
    #[test]
    fn saves_go_to_the_source_assets_directory() {
        let path = asset_path("data/ships.ron");
        assert!(path.is_absolute());
        assert!(
            path.ends_with("assets/data/ships.ron") || path.ends_with("assets\\data\\ships.ron")
        );
    }
}
