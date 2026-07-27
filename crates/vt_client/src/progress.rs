//! What survives a run ending.
//!
//! v&t is a single-sitting arcade game: a run is a few minutes, and when the
//! ship goes down everything about it is gone. That is the design and it stays
//! the design. But a player who has sailed forty runs has *done* something,
//! and until now the game forgot all of it the moment the tab closed.
//!
//! So this is the whole of v&t's durable state: a career tally on the title
//! card, and which loadout you last flew. Deliberately not: unlocks, currency,
//! or anything a run can spend — v&t has no meta-progression and this is not
//! the beginning of one. A remembered fit is a *preference*, not a reward:
//! nothing earns it, every option is available from the first run, and all it
//! saves you is re-picking the same three chips every time.
//!
//! Two slots rather than one field, because they are different kinds of thing
//! and fail differently. A corrupt career should not cost you your loadout, and
//! a catalogue edit that invalidates a saved index should not put the career
//! tally through a migration.
//!
//! # Why this game and not a replay
//!
//! v&t drives [`vellum_save::Progress`] and can never drive `Run`. Its input
//! is a continuous analog stick sampled at 64 Hz, so a "command log" would be
//! an input recording, not a log of decisions — the fleet's replay contract
//! does not apply here and pretending otherwise would cost a format nobody
//! could honour. That is exactly why v&t was chosen to prove the `Progress`
//! half: it is the game that has only that half.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use vellum_save::Progress;

use vt_sim::prelude::{Encounter, Outcome, Plunder};

use crate::data::{LoadoutCatalogue, SelectedLoadout};

/// The career slot. One career, no save selection — there is nothing to choose
/// between.
const SLOT: &str = "career";

/// The loadout slot: which chips were lit when you last cast off.
const LOADOUT_SLOT: &str = "loadout";

/// The `localStorage` key prefix. Namespaced because a GitHub Pages account
/// serves every game in the fleet from one origin, and two games sharing a
/// key would overwrite each other's saves.
#[cfg(target_arch = "wasm32")]
const NAMESPACE: &str = "void-and-thunder";

/// Everything the game remembers between runs.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Career {
    /// Runs that reached an ending. A quit mid-run is not a run.
    pub runs: u32,
    /// Runs that cleared every wave.
    pub victories: u32,
    /// The deepest wave *reached*, won or lost — the same figure the
    /// end-of-run banner reports, for the same reason.
    pub deepest_wave: u32,
    pub ships_boarded: u64,
}

impl Progress for Career {
    const FORMAT: u32 = 1;
    // No migrations: format 1 is the first. An added field needs none — serde's
    // default covers it — so this list stays empty until a field is
    // restructured or removed.
}

/// The remembered fit. Indices into the catalogue, which is why loading one
/// clamps: a saved index outlives the list it pointed into.
impl Progress for SelectedLoadout {
    const FORMAT: u32 = 1;
}

/// The store, chosen at compile time because the two targets have nothing in
/// common: a directory on a desktop, `localStorage` in a browser.
#[cfg(not(target_arch = "wasm32"))]
type Backend = vellum_save::FileStore;
#[cfg(target_arch = "wasm32")]
type Backend = vellum_save::LocalStorage;

#[derive(Resource)]
struct Saves(Backend);

#[cfg(not(target_arch = "wasm32"))]
fn backend() -> Backend {
    // Beside the executable rather than in a platform data directory: v&t is
    // played from a folder or from a browser, and a stray `saves/` next to the
    // binary is easier to find, copy and delete than a path four levels inside
    // AppData. Falls back to the working directory if the executable's home
    // cannot be determined.
    let root = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("saves")))
        .unwrap_or_else(|| std::path::PathBuf::from("saves"));
    vellum_save::FileStore::new(root)
}

#[cfg(target_arch = "wasm32")]
fn backend() -> Backend {
    vellum_save::LocalStorage::new(NAMESPACE)
}

pub struct ProgressPlugin;

impl Plugin for ProgressPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Career>()
            .insert_resource(Saves(backend()))
            .add_systems(Startup, (load, load_loadout))
            // Written on change rather than on cast-off, so the choice survives
            // closing the game from the title card without ever starting a run.
            .add_systems(Update, save_loadout)
            .add_systems(OnEnter(crate::GameState::GameOver), record);
    }
}

/// Read the career at startup.
///
/// Every failure here is survivable and none of them stop the game: a first
/// run has no save, a browser in private mode has no storage, and a corrupt
/// file is worth a warning rather than a refusal to start. The career resets
/// to zero and the player can still play — which is the whole reason progress
/// migrates and runs refuse.
fn load(mut career: ResMut<Career>, saves: Res<Saves>) {
    match vellum_save::load::<Career, _>(&saves.0, SLOT) {
        Ok(Some(loaded)) => *career = loaded,
        Ok(None) => {}
        Err(Ok(error)) => warn!("could not read the saved career: {error}"),
        Err(Err(error)) => warn!("could not reach saved-game storage: {error}"),
    }
}

/// Read the remembered fit at startup, clamped to what the catalogue still
/// offers.
///
/// The clamp is the whole reason this is not a one-liner: the save holds
/// *indices*, and a catalogue that has since lost an option would otherwise
/// leave a slot pointing at nothing — which reads on the card as a row with no
/// chip lit, and spawns whatever the fallback happens to be.
fn load_loadout(
    mut fit: ResMut<SelectedLoadout>,
    catalogue: Res<LoadoutCatalogue>,
    saves: Res<Saves>,
) {
    // The catalogue asset has not landed yet at `Startup`, so this clamps
    // against the compiled-in default. That is the same list the shipped file
    // holds — a test asserts it — and a hand-shrunk file is caught downstream
    // anyway: `LoadoutCatalogue::fit` falls back to the first option rather
    // than flying a ship with an empty slot.
    match vellum_save::load::<SelectedLoadout, _>(&saves.0, LOADOUT_SLOT) {
        Ok(Some(loaded)) => *fit = clamp_to(loaded, &catalogue),
        Ok(None) => {}
        Err(Ok(error)) => warn!("could not read the saved loadout: {error}"),
        Err(Err(error)) => warn!("could not reach saved-game storage: {error}"),
    }
}

/// A selection with every slot inside the catalogue's bounds.
fn clamp_to(fit: SelectedLoadout, catalogue: &LoadoutCatalogue) -> SelectedLoadout {
    let [broadsides, batteries, specials] = catalogue.counts();
    let within = |index: usize, count: usize| if index < count { index } else { 0 };
    SelectedLoadout {
        broadside: within(fit.broadside, broadsides),
        battery: within(fit.battery, batteries),
        special: within(fit.special, specials),
    }
}

/// Write the fit out whenever it changes.
///
/// Change detection rather than a call at the point of selection: the chips are
/// not the only thing that may ever write this, and a save that depends on
/// remembering to call it is a save that eventually stops happening. Bevy fires
/// `is_changed` once on insertion too, which harmlessly rewrites the default on
/// the first frame of a fresh install.
fn save_loadout(fit: Res<SelectedLoadout>, saves: Res<Saves>) {
    if !fit.is_changed() {
        return;
    }
    if let Err(error) = vellum_save::save(&saves.0, LOADOUT_SLOT, &*fit) {
        warn!("could not save the loadout: {error}");
    }
}

/// Fold the finished run into the career and write it out.
///
/// On entering game-over, which happens exactly once per run: the state
/// machine moves here when the encounter resolves, and a restart leaves and
/// re-enters. Doing it on the *transition* rather than every frame of the
/// game-over screen is what keeps a player staring at the banner from
/// counting the same run twice.
fn record(
    mut career: ResMut<Career>,
    saves: Res<Saves>,
    encounter: Res<Encounter>,
    plunder: Res<Plunder>,
) {
    career.runs += 1;
    if encounter.outcome == Outcome::Cleared {
        career.victories += 1;
    }
    career.deepest_wave = career.deepest_wave.max(encounter.wave.max(1));
    career.ships_boarded += u64::from(plunder.ships_boarded);

    if let Err(error) = vellum_save::save(&saves.0, SLOT, &*career) {
        warn!("could not save the career: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The career round-trips through the same encode/decode path the game
    /// uses, with no store involved.
    #[test]
    fn a_career_round_trips() {
        let career = Career {
            runs: 41,
            victories: 3,
            deepest_wave: 9,
            ships_boarded: 118,
        };
        let stored =
            vellum_save::encode::<Career, core::convert::Infallible>(&career).expect("encodes");
        assert_eq!(
            vellum_save::decode::<Career>(&stored).expect("decodes"),
            career
        );
    }

    /// A field added later must not need a migration, because that is the
    /// promise the empty migration list is making.
    #[test]
    fn an_older_career_loads_with_the_new_field_defaulted() {
        #[derive(Default, Serialize, Deserialize)]
        #[serde(default)]
        struct Older {
            runs: u32,
            victories: u32,
            deepest_wave: u32,
        }
        impl Progress for Older {
            const FORMAT: u32 = 1;
        }

        let stored = vellum_save::encode::<Older, core::convert::Infallible>(&Older {
            runs: 5,
            victories: 1,
            deepest_wave: 4,
        })
        .expect("encodes");

        let loaded = vellum_save::decode::<Career>(&stored).expect("decodes");
        assert_eq!(loaded.runs, 5);
        assert_eq!(loaded.ships_boarded, 0);
    }

    /// The remembered fit round-trips through the same path the game uses.
    #[test]
    fn a_loadout_round_trips() {
        let fit = SelectedLoadout {
            broadside: 1,
            battery: 2,
            special: 1,
        };
        let stored = vellum_save::encode::<SelectedLoadout, core::convert::Infallible>(&fit)
            .expect("encodes");
        assert_eq!(
            vellum_save::decode::<SelectedLoadout>(&stored).expect("decodes"),
            fit
        );
    }

    /// The save holds *indices*, so it can outlive the list it points into. A
    /// selection left over from a bigger catalogue must come back as something
    /// fittable rather than as a slot pointing at nothing.
    #[test]
    fn a_selection_from_a_bigger_catalogue_is_clamped_on_load() {
        let catalogue = LoadoutCatalogue::default();
        let stale = SelectedLoadout {
            broadside: 99,
            battery: 99,
            special: 99,
        };
        assert_eq!(clamp_to(stale, &catalogue), SelectedLoadout::default());
    }

    /// ...and a selection that is still in range is left exactly alone.
    #[test]
    fn a_selection_that_still_fits_survives_the_clamp() {
        let catalogue = LoadoutCatalogue::default();
        let fit = SelectedLoadout {
            broadside: 1,
            battery: 2,
            special: 2,
        };
        assert_eq!(clamp_to(fit, &catalogue), fit);
    }

    /// A hand-edited save is refused rather than loaded, so a tampered career
    /// cannot quietly become the real one.
    #[test]
    fn a_tampered_career_is_refused() {
        let stored = vellum_save::encode::<Career, core::convert::Infallible>(&Career {
            runs: 1,
            ..Career::default()
        })
        .expect("encodes");
        let tampered = stored.replace("runs:1", "runs:9999");
        assert!(vellum_save::decode::<Career>(&tampered).is_err());
    }
}
