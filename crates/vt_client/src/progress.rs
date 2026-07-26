//! What survives a run ending.
//!
//! v&t is a single-sitting arcade game: a run is a few minutes, and when the
//! ship goes down everything about it is gone. That is the design and it stays
//! the design. But a player who has sailed forty runs has *done* something,
//! and until now the game forgot all of it the moment the tab closed.
//!
//! So this is the whole of v&t's durable state: a career tally, on the title
//! card, and nothing else. Deliberately not: unlocks, currency, or anything a
//! run can spend — v&t has no meta-progression and this is not the beginning
//! of one.
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

/// The slot. One career, no save selection — there is nothing to choose
/// between.
const SLOT: &str = "career";

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
            .add_systems(Startup, load)
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
