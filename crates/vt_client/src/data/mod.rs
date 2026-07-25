//! Loading tuning data from RON, and pushing it into the sim.
//!
//! The split of responsibility is deliberate. `vt_sim` owns the tuning *types*
//! and knows how to be deserialized, but has no `bevy_asset` dependency and
//! never touches a file — that is what keeps its headless `Harness` a bare
//! `World` with no plugins. This module is the other half: it owns the
//! [`Asset`] wrappers, the loaders, the handles, and the systems that copy a
//! loaded asset into the plain `Resource` the sim reads.
//!
//! Because the sim's `Default` impls reproduce the constants exactly, a missing
//! or partial file degrades rather than fails: you get the compiled-in values.
//!
//! With the `hot-reload` feature on, Bevy's file watcher re-runs the load when a
//! file changes on disk and the same apply system picks it up — so an edit in an
//! external editor and an edit from the design panel converge on one code path.
//!
//! ## A trap worth knowing about
//!
//! `trunk`'s dev server answers a request for a missing file with **200 + the
//! index page**, not a 404 (this is also why `AssetMetaCheck::Never` is set in
//! `main`). A mistyped path here therefore surfaces as a RON parse error on line
//! 1 complaining about `<!DOCTYPE`, never as "file not found". Every path lives
//! in [`paths`] for that reason, and load failures log the path they asked for.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use vt_sim::prelude::SimTuning;

/// Every data file the client loads. One place, so a rename can't leave a
/// mistyped path to be discovered as a confusing parse error at runtime.
pub mod paths {
    /// Simulation rules — damage, ranges, AI gains.
    pub const SIM_TUNING: &str = "data/sim.tuning.ron";
}

/// A tuning file, as an asset.
///
/// A newtype because [`SimTuning`] lives in `vt_sim`, which has no `bevy_asset`
/// to derive [`Asset`] from. `#[serde(transparent)]` keeps the wrapper out of
/// the file, so the RON is just the tuning struct's own body.
#[derive(Asset, TypePath, Debug, Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct SimTuningAsset(pub SimTuning);

/// Loads any `serde`-deserializable asset from RON.
///
/// Generic because every data file in this game is "some struct, in RON" — the
/// only thing that differs is which struct. `extensions` is carried per-instance
/// so one type can serve them all.
///
/// Note that the asset server resolves a *typed* `load::<A>()` by asset type
/// before it ever looks at the extension, so two data files may share an
/// extension (`sim.tuning.ron` and `feel.tuning.ron` both end `tuning.ron`)
/// without ambiguity.
#[derive(TypePath)]
pub struct RonAssetLoader<A: Asset + DeserializeOwned> {
    extensions: &'static [&'static str],
    marker: PhantomData<fn() -> A>,
}

impl<A: Asset + DeserializeOwned> RonAssetLoader<A> {
    pub fn new(extensions: &'static [&'static str]) -> Self {
        Self {
            extensions,
            marker: PhantomData,
        }
    }
}

/// Why a data file couldn't be loaded.
///
/// The path is carried in the message because the failure that actually happens
/// in practice — a mistyped path answered by a dev server with the index page —
/// reads as a nonsense RON error unless you can see which file it was.
#[derive(Debug)]
pub enum RonLoadError {
    Io {
        path: String,
        source: std::io::Error,
    },
    Ron {
        path: String,
        source: ron::error::SpannedError,
    },
}

impl std::fmt::Display for RonLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not read {path}: {source}"),
            Self::Ron { path, source } => write!(
                f,
                "could not parse {path} as RON: {source} (if this mentions \
                 `<!DOCTYPE`, the file is missing and a dev server answered \
                 with the index page instead of a 404)"
            ),
        }
    }
}

impl std::error::Error for RonLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Ron { source, .. } => Some(source),
        }
    }
}

impl<A: Asset + DeserializeOwned> AssetLoader for RonAssetLoader<A> {
    type Asset = A;
    type Settings = ();
    type Error = RonLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<A, RonLoadError> {
        let path = ctx.path().to_string();
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| RonLoadError::Io {
                path: path.clone(),
                source,
            })?;
        ron::de::from_bytes(&bytes).map_err(|source| RonLoadError::Ron { path, source })
    }

    fn extensions(&self) -> &[&str] {
        self.extensions
    }
}

/// Live handles to the data files.
///
/// Held for their whole life on purpose: dropping a handle would let the asset
/// be unloaded, and with it the hot-reload watch on that file.
#[derive(Resource, Default)]
pub struct DataHandles {
    pub sim_tuning: Handle<SimTuningAsset>,
}

/// Loads the tuning data and keeps the sim's resources in step with it.
pub struct DataPlugin;

impl Plugin for DataPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SimTuningAsset>()
            .register_asset_loader(RonAssetLoader::<SimTuningAsset>::new(&["tuning.ron"]))
            .init_resource::<DataHandles>()
            .add_systems(Startup, begin_load)
            .add_systems(Update, apply_sim_tuning);
    }
}

/// Kick off the loads. Nothing waits on them here — the sim runs on its
/// `Default` tuning until the files arrive, which is the same thing that
/// happens if they are missing entirely.
fn begin_load(server: Res<AssetServer>, mut handles: ResMut<DataHandles>) {
    handles.sim_tuning = server.load(paths::SIM_TUNING);
}

/// Copy loaded (or reloaded) sim tuning into the resource the sim reads.
///
/// The equality guard is what stops a save/watch feedback loop: writing the file
/// from the design panel makes the watcher fire, which re-parses a file that
/// already matches what's in memory. Comparing values rather than timestamps
/// means "did this change" is answered by the only thing that matters.
fn apply_sim_tuning(
    mut events: MessageReader<AssetEvent<SimTuningAsset>>,
    assets: Res<Assets<SimTuningAsset>>,
    mut tuning: ResMut<SimTuning>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(loaded) = assets.get(*id) else {
            continue;
        };
        if *tuning == loaded.0 {
            continue; // our own save came back around; nothing actually moved
        }
        *tuning = loaded.0;
        info!("sim tuning reloaded from {}", paths::SIM_TUNING);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped file must be exactly the compiled-in defaults. This is what
    /// stops the data and the `Default` impls drifting apart: change one without
    /// the other and CI says so.
    #[test]
    fn shipped_sim_tuning_matches_the_defaults() {
        let text = include_str!("../../assets/data/sim.tuning.ron");
        let parsed: SimTuning = ron::from_str(text).expect("sim.tuning.ron should parse");
        assert_eq!(
            parsed,
            SimTuning::default(),
            "assets/data/sim.tuning.ron has drifted from SimTuning::default()"
        );
    }
}
