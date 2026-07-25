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

pub mod feel;
pub mod rig;
pub mod scenario;
pub mod ships;

// The scenario corpus is a test instrument: authored scenarios driven
// headlessly by the AI pilot, batched and reported via vellum-corpus.
#[cfg(test)]
mod corpus;

// The composition bridge: ship classes as a vellum-compose template catalog,
// proven equivalent to the typed path against the real ships.ron. The design
// panel's sparse save path consumes vellum-compose directly; this bridge is
// the test instrument that keeps value-space and the typed loader honest
// with each other (including for every sparse file the panel writes).
#[cfg(test)]
pub(crate) mod compose;

pub use feel::FeelTuning;
pub use rig::ModelRigs;
pub use scenario::{director_for, spawn_scenario, Scenario};
pub use ships::{set_director, ShipTable};

/// Every data file the client loads. One place, so a rename can't leave a
/// mistyped path to be discovered as a confusing parse error at runtime.
pub mod paths {
    /// Simulation rules — damage, ranges, AI gains.
    pub const SIM_TUNING: &str = "data/sim.tuning.ron";
    /// How the game feels — bullet-time, shake, trails, camera.
    pub const FEEL_TUNING: &str = "data/feel.tuning.ron";
    /// The ship classes every ship is an instance of.
    pub const SHIPS: &str = "data/ships.ron";
    /// The normal encounter: a player, and three escalating waves.
    pub const SKIRMISH: &str = "data/scenarios/skirmish.scn.ron";
    /// One stationary, inert, invulnerable target — somewhere to tune against.
    pub const TEST_RANGE: &str = "data/scenarios/test_range.scn.ron";

    /// Every scenario, as `(label, path)`. The design panel lists these.
    pub const SCENARIOS: &[(&str, &str)] = &[("Skirmish", SKIRMISH), ("Test Range", TEST_RANGE)];

    /// Every ship model's rig sidecar, as `(model path, sidecar path)` — the
    /// sidecar sits beside its `.glb`, per the fleet composition pipeline's
    /// convention.
    pub const MODEL_RIGS: &[(&str, &str)] = &[
        ("models/bob.glb", "models/bob.model.ron"),
        ("models/challenger.glb", "models/challenger.model.ron"),
        ("models/dispatcher.glb", "models/dispatcher.model.ron"),
        ("models/executioner.glb", "models/executioner.model.ron"),
        ("models/imperial.glb", "models/imperial.model.ron"),
    ];
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
    pub feel: Handle<FeelTuning>,
    pub ships: Handle<ShipTable>,
    /// Both scenarios stay loaded so switching to the test range is instant and
    /// a hot-reload watch stays on each.
    pub scenarios: Vec<(&'static str, Handle<Scenario>)>,
    /// One rig per ship model, keyed by *model* path so the apply system can
    /// file a loaded rig under the model it describes.
    pub rigs: Vec<(&'static str, Handle<rig::ModelRig>)>,
}

impl DataHandles {
    /// The handle for a scenario path, if it is one we loaded.
    pub fn scenario(&self, path: &str) -> Option<&Handle<Scenario>> {
        self.scenarios
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, h)| h)
    }
}

/// Which scenario the next run uses. Set by the title card and the design panel.
#[derive(Resource, Debug, Clone)]
pub struct SelectedScenario(pub &'static str);

impl Default for SelectedScenario {
    fn default() -> Self {
        Self(paths::SKIRMISH)
    }
}

/// The scenario currently in play, resolved. Held as a resource so a restart can
/// replay it and the panel can show its name without touching the asset store.
#[derive(Resource, Debug, Clone, Default)]
pub struct ActiveScenario(pub Scenario);

/// Loads the tuning and scenario data and keeps the sim's resources in step.
pub struct DataPlugin;

impl Plugin for DataPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<SimTuningAsset>()
            .init_asset::<FeelTuning>()
            .init_asset::<ShipTable>()
            .init_asset::<Scenario>()
            .register_asset_loader(RonAssetLoader::<SimTuningAsset>::new(&["tuning.ron"]))
            .register_asset_loader(RonAssetLoader::<FeelTuning>::new(&["tuning.ron"]))
            .register_asset_loader(RonAssetLoader::<ShipTable>::new(&["ron"]))
            .register_asset_loader(RonAssetLoader::<Scenario>::new(&["scn.ron"]))
            .init_asset::<rig::ModelRig>()
            .register_asset_loader(RonAssetLoader::<rig::ModelRig>::new(&["model.ron"]))
            .init_resource::<rig::ModelRigs>()
            .init_resource::<DataHandles>()
            .init_resource::<SelectedScenario>()
            .init_resource::<ActiveScenario>()
            // The class table is a Resource as well as an Asset: the panel edits
            // the resource, and the loader copies into it. One authoritative
            // copy, whichever end the edit came from.
            .init_resource::<ShipTable>()
            .init_resource::<FeelTuning>()
            .add_systems(Startup, begin_load)
            .add_systems(
                Update,
                (
                    apply_sim_tuning,
                    apply_feel_tuning,
                    apply_ship_table,
                    apply_model_rigs,
                ),
            );
    }
}

/// Kick off the loads. Nothing waits on the tuning — the sim runs on its
/// `Default` values until the file arrives, the same as if it were missing. The
/// *scenario* is waited on, because spawning needs the class table (see
/// `session::await_data`).
fn begin_load(server: Res<AssetServer>, mut handles: ResMut<DataHandles>) {
    handles.sim_tuning = server.load(paths::SIM_TUNING);
    handles.feel = server.load(paths::FEEL_TUNING);
    handles.ships = server.load(paths::SHIPS);
    handles.scenarios = paths::SCENARIOS
        .iter()
        .map(|(_, path)| (*path, server.load(*path)))
        .collect();
    handles.rigs = paths::MODEL_RIGS
        .iter()
        .map(|(model, sidecar)| (*model, server.load(*sidecar)))
        .collect();
}

/// Copy each loaded (or reloaded) model rig into the [`rig::ModelRigs`]
/// resource, keyed by the model it describes.
fn apply_model_rigs(
    mut events: MessageReader<AssetEvent<rig::ModelRig>>,
    assets: Res<Assets<rig::ModelRig>>,
    handles: Res<DataHandles>,
    mut rigs: ResMut<rig::ModelRigs>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(loaded) = assets.get(*id) else {
            continue;
        };
        let Some((model, _)) = handles.rigs.iter().find(|(_, handle)| handle.id() == *id) else {
            continue;
        };
        if rigs.get(model) == Some(loaded) {
            continue; // our own save came back around
        }
        rigs.insert(model, loaded.clone());
        info!("model rig reloaded for {model}");
    }
}

/// Copy loaded (or reloaded) feel tuning into the resource the client reads.
fn apply_feel_tuning(
    mut events: MessageReader<AssetEvent<FeelTuning>>,
    assets: Res<Assets<FeelTuning>>,
    mut feel: ResMut<FeelTuning>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(loaded) = assets.get(*id) else {
            continue;
        };
        if *feel == *loaded {
            continue; // our own save came back around
        }
        *feel = *loaded;
        info!("feel tuning reloaded from {}", paths::FEEL_TUNING);
    }
}

/// Copy a loaded (or reloaded) class table into the resource everything reads.
fn apply_ship_table(
    mut events: MessageReader<AssetEvent<ShipTable>>,
    assets: Res<Assets<ShipTable>>,
    mut table: ResMut<ShipTable>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(loaded) = assets.get(*id) else {
            continue;
        };
        if *table == *loaded {
            continue; // our own save came back around
        }
        *table = loaded.clone();
        info!("ship classes reloaded from {}", paths::SHIPS);
    }
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

    /// The shipped classes must equal the fallback table, so a missing
    /// `ships.ron` plays identically rather than subtly differently.
    #[test]
    fn shipped_ships_match_the_default_table() {
        let text = include_str!("../../assets/data/ships.ron");
        let parsed: ShipTable = ron::from_str(text).expect("ships.ron should parse");
        assert_eq!(
            parsed,
            ShipTable::default(),
            "assets/data/ships.ron has drifted from ShipTable::default()"
        );
    }

    /// The skirmish is the encounter the game shipped with, as data.
    #[test]
    fn shipped_skirmish_matches_the_default_scenario() {
        let text = include_str!("../../assets/data/scenarios/skirmish.scn.ron");
        let parsed: Scenario = ron::from_str(text).expect("skirmish.scn.ron should parse");
        assert_eq!(
            parsed,
            Scenario::default(),
            "assets/data/scenarios/skirmish.scn.ron has drifted from Scenario::default()"
        );
    }

    /// The test range is the point of the whole exercise: one target that never
    /// dies, never fires and never moves, with no waves to interrupt tuning.
    #[test]
    fn the_test_range_is_one_inert_invulnerable_target() {
        let text = include_str!("../../assets/data/scenarios/test_range.scn.ron");
        let parsed: Scenario = ron::from_str(text).expect("test_range.scn.ron should parse");

        assert!(parsed.director.is_none(), "the range must send no waves");
        assert_eq!(parsed.enemies.len(), 1, "exactly one target");

        let target = &parsed.enemies[0];
        assert!(target.flags.invulnerable, "the target must survive tuning");
        assert!(target.flags.inert, "the target must not shoot back");
        assert!(target.flags.anchored, "the target must hold its mark");

        // Both classes it names must exist, or the range spawns nothing.
        let table = ShipTable::default();
        assert!(table.find(&parsed.player.class).is_some());
        assert!(table.find(&target.class).is_some());
    }
}
