//! Player-facing text.
//!
//! Every line a player reads lives in `assets/strings/en.csv` and is reached
//! by id. Almost all of this game's text is on the HUD page, so the table's
//! main job is to be [served to it as JSON](crate::strings::TABLE_JSON) —
//! but the same table answers Rust lookups through [`tr!`] and [`trf!`], so
//! text that moves into Rust later needs no second mechanism.
//!
//! **The design panel is deliberately not localised.** It is a developer
//! tool; its labels are part of the workshop, not the game, and a translator
//! asked to render "Save to ships.ron" would rightly wonder what they had
//! been handed.

use std::sync::OnceLock;

use vellum_strings::{Locale, Table};

/// The authored table, embedded so native and web read the same bytes.
const EN_CSV: &str = include_str!("../assets/strings/en.csv");

/// The same table as JSON, written by `build.rs` with the same parser.
///
/// Embedded as well as shipped, so a page that cannot fetch it — a file://
/// open, a stripped deployment — can still be handed the strings over the
/// existing HUD bridge instead of falling back to authored English.
pub const TABLE_JSON: &str = include_str!("../assets/strings/en.json");

/// The table, parsed once.
pub fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        // A malformed table is an authoring error caught in `build.rs` before
        // this can ever run, so reaching the panic means the embedded bytes
        // and the built bytes disagree — which is worth stopping for.
        Table::parse(Locale::ENGLISH, EN_CSV).unwrap_or_else(|errors| {
            for error in &errors {
                error!("en.csv: {error}");
            }
            panic!("the embedded string table does not parse");
        })
    })
}

use bevy::prelude::error;

/// Route missed lookups into Bevy's logger, so a release build leaves a trace
/// rather than only a marker nobody was looking at.
pub fn install_miss_reporting() {
    vellum_strings::on_missing(|id| bevy::prelude::warn!("missing string: `{id}`"));
}

/// The text for an id.
#[macro_export]
macro_rules! tr {
    ($id:literal) => {
        $crate::strings::table().text($id)
    };
}

/// The text for an id, with its `{named}` slots filled.
#[macro_export]
macro_rules! trf {
    ($id:literal, $($key:ident = $value:expr),+ $(,)?) => {
        $crate::strings::table().format($id, &[$((stringify!($key), &*$value.to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use vellum_strings::{audit, AuditInput};

    fn manifest(relative: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn the_authored_table_parses() {
        let table = table();
        assert!(!table.is_empty());
        assert_eq!(table.text("hud.stat.wave"), "WAVE");
    }

    /// The check that makes renaming a string safe, across all three
    /// languages this game writes text in: Rust lookups, the HUD page's
    /// `t(...)` calls, and its `data-i18n` attributes.
    ///
    /// Almost every id is a literal somewhere. The exception is the loadout
    /// menu: the card builds `t(option.id + ".name")` from ids the *host* sent
    /// it, so no scan can see them. Those are handed to the audit explicitly —
    /// which keeps the orphan half exact rather than blinding it with a prefix,
    /// so deleting an option still reports its rows as unreachable.
    ///
    /// The slot *labels* stay literal in the page for the same reason — see the
    /// SLOTS table in hud.html.
    #[test]
    fn every_id_is_defined_and_every_row_is_reached() {
        let catalogue = crate::data::LoadoutCatalogue::default();
        let options = catalogue
            .broadsides
            .iter()
            .map(|o| o.id.clone())
            .chain(catalogue.batteries.iter().map(|o| o.id.clone()))
            .chain(catalogue.specials.iter().map(|o| o.id.clone()))
            .flat_map(|id| [format!("{id}.name"), format!("{id}.desc")]);
        let report = audit(
            table(),
            AuditInput::new(&[manifest("src"), manifest("assets/ui")])
                // This module's own macro definitions name `tr!(` and
                // `trf!(` without looking anything up.
                .skip("strings.rs")
                .derived(options),
        );
        assert!(report.files_scanned > 0, "the audit scanned nothing");
        assert!(report.ok(), "\n{report}");
    }

    /// The JSON the page fetches must be the table, not a stale artifact from
    /// an earlier edit — build.rs regenerates it, and this is what notices if
    /// it did not.
    #[test]
    fn the_emitted_json_matches_the_table() {
        assert_eq!(TABLE_JSON, table().to_json());
    }
}
