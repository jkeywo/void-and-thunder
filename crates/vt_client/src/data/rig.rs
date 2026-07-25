//! Model sidecars: authored data about an asset, next to the asset.
//!
//! Each ship `.glb` under `assets/models/` is paired with a
//! `<model>.model.ron` sidecar describing its *rig* — the named geometry
//! presentation reads. This is the fleet composition pipeline's sidecar
//! convention (vellum `docs/handbook/composition.md`), grown in
//! project-phoenix-v2 as `*.model.toml` and landed here first in this game's
//! own format.
//!
//! The first rig datum is the engine trail anchors. Before sidecars, every
//! hull streamed its plumes from one *global* nacelle pair in the feel
//! tuning — the same offsets for a 46KB sloop and a 140KB executioner. A
//! sidecar gives each model its own anchors; a model without one (or with
//! an empty list) keeps the global pair, so a missing sidecar changes
//! nothing.

use bevy::prelude::*;
use std::collections::BTreeMap;

/// The rig of one ship model.
#[derive(
    Asset, TypePath, Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize,
)]
#[serde(default)]
pub struct ModelRig {
    /// Where engine ribbons stream from, in the model's local frame (+X bow,
    /// +Y port, +Z up). Empty means "use the global feel-tuning nacelle
    /// pair" — the pre-sidecar behaviour.
    pub trail_anchors: Vec<Vec3>,
}

/// Every loaded rig, keyed by its model's path (`models/challenger.glb`).
/// BTreeMap so any iteration or report over rigs is deterministic.
#[derive(Resource, Debug, Default)]
pub struct ModelRigs {
    rigs: BTreeMap<String, ModelRig>,
}

impl ModelRigs {
    /// The authored trail anchors for a model, or `None` when the model has
    /// no sidecar (or an empty list) and the caller should fall back to the
    /// global nacelle pair.
    pub fn trail_anchors(&self, model_path: &str) -> Option<&[Vec3]> {
        let anchors = &self.rigs.get(model_path)?.trail_anchors;
        if anchors.is_empty() {
            None
        } else {
            Some(anchors)
        }
    }

    pub(crate) fn insert(&mut self, model_path: &str, rig: ModelRig) {
        self.rigs.insert(model_path.to_owned(), rig);
    }

    pub(crate) fn get(&self, model_path: &str) -> Option<&ModelRig> {
        self.rigs.get(model_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped model must have a sidecar, every sidecar must parse,
    /// and — the property the wiring depends on — each must carry at least
    /// one trail anchor. A model that *wants* the global fallback would
    /// simply not ship a sidecar.
    #[test]
    fn every_shipped_sidecar_parses_and_rigs_its_model() {
        for (model, sidecar) in crate::data::paths::MODEL_RIGS {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join(sidecar);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{model} has no sidecar at {}: {e}", path.display()));
            let rig: ModelRig =
                ron::from_str(&text).unwrap_or_else(|e| panic!("{sidecar} does not parse: {e}"));
            assert!(
                !rig.trail_anchors.is_empty(),
                "{sidecar} rigs no trail anchors; a model wanting the global \
                 fallback should ship no sidecar at all"
            );
        }
    }

    #[test]
    fn a_missing_or_empty_rig_falls_back() {
        let mut rigs = ModelRigs::default();
        assert!(rigs.trail_anchors("models/none.glb").is_none());
        rigs.insert("models/none.glb", ModelRig::default());
        assert!(
            rigs.trail_anchors("models/none.glb").is_none(),
            "an empty anchor list means the global pair, not zero trails"
        );
    }
}
