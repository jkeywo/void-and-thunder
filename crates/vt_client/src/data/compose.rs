//! The composition bridge: ship classes as a `vellum-compose` template
//! catalog, proven equivalent to the typed path.
//!
//! This is the data editor's foundation, laid before the editor exists. The
//! editor will edit *values* — pick a class, override a field, watch every
//! ship wearing it follow — and value-level composition is what
//! `vellum-compose` does. What this module proves, with the real authored
//! `ships.ron`, is that composing in value-space and deserializing agrees
//! exactly with the typed `ShipTable` path the runtime uses today. When the
//! editor arrives it builds on [`catalog_from_ships_ron`] and inherits that
//! equivalence instead of re-earning it.
//!
//! Test-gated until then: the runtime keeps its typed path, and dead code is
//! not the fleet's way of reserving a seat.

use vellum_compose::{Catalog, ComposeError};

use super::ships::ShipTable;

/// The authored class file, shared with the typed loader.
pub(crate) const SHIPS_RON: &str = include_str!("../../assets/data/ships.ron");

/// Build a template catalog from a `ships.ron` text: one template per named
/// class, holding the class's authored value exactly as written.
pub(crate) fn catalog_from_ships_ron(text: &str) -> Result<Catalog, ComposeError> {
    use ron::Value;
    let table = vellum_compose::parse(text)?;
    let Value::Map(table_map) = &table else {
        return Err(ComposeError::NotAMap("ships.ron root"));
    };
    let Some(Value::Seq(classes)) = table_map.get(&Value::String("classes".into())) else {
        return Err(ComposeError::NotAMap("ships.ron classes"));
    };
    let mut catalog = Catalog::new();
    for entry in classes {
        let Value::Map(entry_map) = entry else {
            return Err(ComposeError::NotAMap("ships.ron class entry"));
        };
        let Some(Value::String(name)) = entry_map.get(&Value::String("name".into())) else {
            return Err(ComposeError::NotAMap("ships.ron class name"));
        };
        let Some(class) = entry_map.get(&Value::String("class".into())) else {
            return Err(ComposeError::NotAMap("ships.ron class body"));
        };
        catalog.insert(name, class.clone());
    }
    Ok(catalog)
}

/// The typed table from the same text — the runtime's current path.
pub(crate) fn typed_table(text: &str) -> ShipTable {
    ron::from_str(text).expect("ships.ron parses as a ShipTable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ships::ShipClass;
    use vellum_compose::{extract, parse};

    /// The load-bearing claim: for every authored class, resolving the
    /// template and extracting it typed produces exactly what the typed
    /// loader produces — serde defaults, omitted fields and all. The editor
    /// builds on this equivalence rather than re-earning it.
    #[test]
    fn every_authored_class_composes_equal_to_the_typed_path() {
        let catalog = catalog_from_ships_ron(SHIPS_RON).expect("catalog builds");
        let table = typed_table(SHIPS_RON);
        assert!(!table.classes.is_empty());
        for named in &table.classes {
            let template = catalog
                .resolve(&named.name)
                .unwrap_or_else(|_| panic!("catalog is missing '{}'", named.name));
            let composed: ShipClass =
                extract(template.clone()).expect("an authored class extracts typed");
            assert_eq!(
                composed, named.class,
                "value-space and the typed loader disagree about '{}'",
                named.name
            );
        }
    }

    /// A per-instance override — the scenario scheme's `hull:` field, in
    /// value-space: named field replaced, every other authored value kept.
    #[test]
    fn a_hull_override_composes_like_a_placed_ship() {
        let catalog = catalog_from_ships_ron(SHIPS_RON).expect("catalog builds");
        let table = typed_table(SHIPS_RON);
        let named = &table.classes[0];

        let overridden: ShipClass = extract(
            catalog
                .instantiate(&named.name, &parse("( hull: 40.0 )").unwrap())
                .expect("instantiation composes"),
        )
        .expect("the composed instance extracts typed");

        // Exactly what spawn_placed's `placed.hull.unwrap_or(class.hull)`
        // produces, reached through value-space instead.
        assert_eq!(overridden.hull, 40.0);
        assert_eq!(overridden.stats, named.class.stats, "untouched fields kept");
        assert_eq!(overridden.loadout, named.class.loadout);
    }

    /// Unknown classes fail loudly with the name — the same authoring error
    /// spawn_placed reports today, caught one layer earlier.
    #[test]
    fn unknown_templates_are_named_in_the_error() {
        let catalog = catalog_from_ships_ron(SHIPS_RON).expect("catalog builds");
        let missing = catalog.resolve("dreadnought_that_never_was");
        assert!(matches!(
            missing,
            Err(vellum_compose::ComposeError::UnknownTemplate(name))
                if name == "dreadnought_that_never_was"
        ));
    }
}
