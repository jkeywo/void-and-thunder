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

use crate::data::ships::ShipClass;
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

/// Write the class table to `ships.ron` — sparsely.
///
/// The authored file's contract is that a class only says how it differs
/// from the compiled-in defaults, and a save must not destroy that: each
/// edited class is diffed against `ShipClass::default()` in value-space
/// (`vellum_compose::diff`) and only the differing fields are written, in
/// the authored struct style (`vellum_compose::write_ron`). A class equal
/// to the defaults writes as `()` — the take-the-defaults idiom itself.
pub fn save_ships(table: &ShipTable) -> Result<(), String> {
    let text = sparse_ships_ron(table).map_err(|e| format!("could not save ship classes — {e}"))?;
    write_text(paths::SHIPS.trim_start_matches("data/"), &text)
        .map_err(|e| format!("could not save ship classes — {e}"))
}

/// The class table as sparse, authored-style RON text.
fn sparse_ships_ron(table: &ShipTable) -> Result<String, String> {
    use ron::Value;
    let defaults = ShipClass::default();
    let mut classes = Vec::new();
    for named in &table.classes {
        let sparse =
            vellum_editor::sparse_override(&defaults, &named.class).map_err(|e| e.to_string())?;
        let mut entry = ron::Map::new();
        entry.insert(
            Value::String("name".into()),
            Value::String(named.name.clone()),
        );
        entry.insert(Value::String("class".into()), sparse);
        classes.push(Value::Map(entry));
    }
    let mut root = ron::Map::new();
    root.insert(Value::String("classes".into()), Value::Seq(classes));
    vellum_editor::write_ron(&Value::Map(root)).map_err(|e| e.to_string())
}

/// Write already-rendered text to `relative` under `assets/`.
fn write_text(relative: &str, text: &str) -> Result<(), String> {
    let path = asset_path(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
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

        let text = sparse_ships_ron(&table).expect("sparse save renders");
        let parsed: ShipTable = ron::from_str(&text).expect("saved RON must parse");

        assert_eq!(parsed, table);
        assert_eq!(parsed.classes[0].class.stats.thrust, 1234.0);
    }

    /// The save is *sparse*: the authored file's contract is that a class
    /// only says how it differs, and editing one field must not entomb
    /// every default alongside it.
    #[test]
    fn a_saved_class_says_only_how_it_differs() {
        let mut table = ShipTable::default();
        table.classes[0].class.hull = 250.0;

        let text = sparse_ships_ron(&table).expect("sparse save renders");
        assert!(text.contains("hull: 250"), "the edit is written:\n{text}");
        assert!(
            !text.contains("collider"),
            "an untouched default must not be entombed in the file:\n{text}"
        );
    }

    /// A value's RON-value form — the composing half of the law below.
    fn value_of<T: Serialize>(value: &T) -> Result<ron::Value, String> {
        vellum_editor::value_of(value).map_err(|e| e.to_string())
    }

    /// The whole class-table pipeline in one law: save sparsely, read it
    /// back through the composition catalog, compose each sparse class over
    /// the defaults, and the result is the table that was saved.
    #[test]
    fn a_sparse_save_composes_back_to_the_edited_table() {
        let mut table = ShipTable::default();
        table.classes[0].class.stats.turn_rate = 9.9;

        let text = sparse_ships_ron(&table).expect("sparse save renders");
        let catalog = crate::data::compose::catalog_from_ships_ron(&text).expect("catalog builds");
        for named in &table.classes {
            let template = catalog.resolve(&named.name).expect("class present");
            let composed: ShipClass = vellum_editor::vellum_compose::extract(
                vellum_editor::vellum_compose::apply(
                    &value_of(&ShipClass::default()).unwrap(),
                    template,
                )
                .expect("defaults + sparse compose"),
            )
            .expect("composed class extracts");
            assert_eq!(composed, named.class);
        }
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
