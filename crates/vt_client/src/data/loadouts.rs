//! The loadout catalogue: what the player may bolt onto their sloop.
//!
//! Deliberately a separate file from `ships.ron`. That one answers "what *is* a
//! house patrol" — the description of a ship the world contains. This one
//! answers "what can the protagonist choose before a run", which is player-facing
//! content with a different lifetime and a different editor: the design panel
//! saves classes sparsely, and folding alternatives into `ShipClass` would make
//! every class carry a list of options it never uses.
//!
//! Three slots, one option each. An option is a *fragment* of a
//! [`ShipLoadout`] — it names only the devices that slot fits, and
//! [`LoadoutCatalogue::fit`] lays the three over a base class's loadout. Which
//! means a class still owns its hull, its shields and its gun *tuning*; the
//! catalogue only decides which devices are carried.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use vt_sim::prelude::{
    Battery, BoostDrive, Broadside, EmpWeapon, FireBarrelRack, MicrowarpDrive, PointDefense,
    ShipLoadout, TorpedoBay,
};

/// Which option is selected in each slot, as indices into the catalogue.
///
/// Indices rather than names so this stays `Copy` and can live in a resource the
/// spawn path reads without allocating. An index that has fallen out of range —
/// a catalogue edited under a running game — resolves to the first option rather
/// than to nothing, so a stale selection still flies a ship.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SelectedLoadout {
    pub broadside: usize,
    pub battery: usize,
    pub special: usize,
}

/// A named broadside: the guns themselves, wholesale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadsideOption {
    /// String-table id, so the menu text goes through `en.csv` like everything
    /// else the HUD says. Never a display string.
    pub id: String,
    pub broadside: Broadside,
}

/// A named battery device, with the pool that runs it.
///
/// Every option carries its own [`Battery`]: a screen that wants a deeper pool
/// than the boost drive is a legitimate thing to author, and tying capacity to
/// the device keeps the whole cost of a choice in one place.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryOption {
    pub id: String,
    pub battery: Battery,
    pub boost: Option<BoostDrive>,
    pub emp: Option<EmpWeapon>,
    pub point_defense: Option<PointDefense>,
}

/// A named special: exactly one of the three.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpecialOption {
    pub id: String,
    pub torpedoes: Option<TorpedoBay>,
    pub microwarp: Option<MicrowarpDrive>,
    pub barrels: Option<FireBarrelRack>,
}

/// Everything the player may fit, per slot.
#[derive(Asset, Resource, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadoutCatalogue {
    pub broadsides: Vec<BroadsideOption>,
    pub batteries: Vec<BatteryOption>,
    pub specials: Vec<SpecialOption>,
}

impl Default for LoadoutCatalogue {
    /// The shipped catalogue, so a missing or unreadable `loadouts.ron` still
    /// gives a playable set of choices rather than a ship with no guns.
    fn default() -> Self {
        Self {
            broadsides: vec![
                BroadsideOption {
                    id: "loadout.long_nines".into(),
                    // One big committed volley on a long reload: the shot the
                    // whole ship is aimed with, and missing it hurts.
                    broadside: Broadside {
                        cooldown: 10.0,
                        ..Broadside::default()
                    },
                },
                BroadsideOption {
                    id: "loadout.carronades".into(),
                    // The brawler's answer: twice the guns at half the weight,
                    // reloading three times as fast, but throwing barely half as
                    // far (180 x 2.5s TTL is ~450, against the nines' ~810). It
                    // trades the decision for a rhythm, and range for presence.
                    broadside: Broadside {
                        cooldown: 3.5,
                        damage: 6.0,
                        muzzle_speed: 180.0,
                        guns: 6,
                        // A wider arc, because at this range you are already
                        // committed to the ship you are alongside.
                        arc: 1.5707964,
                        charge_time: 0.0,
                        ..Broadside::default()
                    },
                },
            ],
            batteries: vec![
                // The disruptor leads because index 0 in every slot is what an
                // untouched `SelectedLoadout` selects, and that has to be the
                // ship the game hands a player who never opens the menu — see
                // `ShipLoadout::player`.
                BatteryOption {
                    id: "loadout.disruptor".into(),
                    emp: Some(EmpWeapon::default()),
                    ..BatteryOption::default()
                },
                BatteryOption {
                    id: "loadout.boost".into(),
                    boost: Some(BoostDrive::default()),
                    ..BatteryOption::default()
                },
                BatteryOption {
                    id: "loadout.point_defense".into(),
                    point_defense: Some(PointDefense::default()),
                    // A deeper pool: the screen is meant to be held up through a
                    // whole pass, where boost is spent in bursts.
                    battery: Battery {
                        charge: 4.5,
                        max: 4.5,
                        ..Battery::default()
                    },
                    ..BatteryOption::default()
                },
            ],
            specials: vec![
                SpecialOption {
                    id: "loadout.torpedoes".into(),
                    torpedoes: Some(TorpedoBay::default()),
                    ..SpecialOption::default()
                },
                SpecialOption {
                    id: "loadout.microwarp".into(),
                    microwarp: Some(MicrowarpDrive {
                        // 20s: warping is an escape, not a manoeuvre.
                        cooldown: 20.0,
                        ..MicrowarpDrive::default()
                    }),
                    ..SpecialOption::default()
                },
                SpecialOption {
                    id: "loadout.barrels".into(),
                    barrels: Some(FireBarrelRack::default()),
                    ..SpecialOption::default()
                },
            ],
        }
    }
}

impl LoadoutCatalogue {
    /// Lay the three selected options over `base`, giving the fit a ship spawns
    /// with.
    ///
    /// `base` is the class's own loadout, so the hull keeps its shields — the
    /// catalogue decides which *devices* are carried, not what kind of ship this
    /// is. Every slot is overwritten wholesale, including the ones the selected
    /// option leaves out: picking the microwarp must actually remove the torpedo
    /// bay, or a fit would only ever accumulate.
    pub fn fit(&self, base: ShipLoadout, selected: SelectedLoadout) -> ShipLoadout {
        let broadside = pick(&self.broadsides, selected.broadside);
        let battery = pick(&self.batteries, selected.battery);
        let special = pick(&self.specials, selected.special);

        ShipLoadout {
            broadside: broadside.map_or(base.broadside, |b| b.broadside),
            shield: base.shield,
            battery: battery.map(|b| b.battery),
            boost: battery.and_then(|b| b.boost),
            emp: battery.and_then(|b| b.emp),
            point_defense: battery.and_then(|b| b.point_defense),
            torpedoes: special.and_then(|s| s.torpedoes),
            microwarp: special.and_then(|s| s.microwarp),
            barrels: special.and_then(|s| s.barrels),
        }
    }

    /// How many options each slot offers, in slot order. The menu needs this to
    /// know when a cycle wraps.
    pub fn counts(&self) -> [usize; 3] {
        [
            self.broadsides.len(),
            self.batteries.len(),
            self.specials.len(),
        ]
    }
}

/// The option at `index`, falling back to the first when the index has gone
/// stale. `None` only when the slot is empty, which is an authoring mistake.
fn pick<T>(options: &[T], index: usize) -> Option<&T> {
    options.get(index).or_else(|| options.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The project's core data discipline: the compiled-in fallback and the
    /// shipped file must describe the *same* catalogue. Asserting against the
    /// file rather than copied literals is what stops the two drifting the next
    /// time an option is retuned.
    #[test]
    fn the_default_catalogue_matches_the_shipped_file() {
        let text = include_str!("../../assets/data/loadouts.ron");
        let shipped: LoadoutCatalogue = ron::from_str(text).expect("loadouts.ron should parse");
        assert_eq!(
            shipped,
            LoadoutCatalogue::default(),
            "assets/data/loadouts.ron has drifted from LoadoutCatalogue::default()"
        );
    }

    /// The default selection must reproduce the ship the game already shipped,
    /// so a player who never opens the menu is handed what they knew.
    #[test]
    fn the_default_selection_is_the_default_fit() {
        let fit =
            LoadoutCatalogue::default().fit(ShipLoadout::player(), SelectedLoadout::default());
        assert_eq!(fit, ShipLoadout::player());
    }

    /// Choosing a slot must *replace* it, not add to it — otherwise every fit
    /// would drift toward carrying everything.
    #[test]
    fn choosing_a_special_removes_the_one_it_replaces() {
        let catalogue = LoadoutCatalogue::default();
        let warp = catalogue.fit(
            ShipLoadout::player(),
            SelectedLoadout {
                special: 1,
                ..SelectedLoadout::default()
            },
        );
        assert!(warp.microwarp.is_some());
        assert!(warp.torpedoes.is_none(), "the tubes must actually come off");
        assert!(warp.barrels.is_none());
    }

    /// Likewise the battery slot, and its pool comes with the device.
    #[test]
    fn choosing_a_battery_device_brings_its_own_pool() {
        let catalogue = LoadoutCatalogue::default();
        let screen = catalogue.fit(
            ShipLoadout::player(),
            SelectedLoadout {
                battery: 2,
                ..SelectedLoadout::default()
            },
        );
        assert!(screen.point_defense.is_some());
        assert!(screen.emp.is_none(), "the disruptor must come off");
        assert!(screen.boost.is_none(), "and so must the boost drive");
        assert_eq!(screen.battery.expect("a pool").max, 4.5);
    }

    /// A class keeps what is *not* in a slot. Shields belong to the hull.
    #[test]
    fn a_fit_never_touches_the_hulls_own_shields() {
        let base = ShipLoadout::player();
        let fit = LoadoutCatalogue::default().fit(
            base,
            SelectedLoadout {
                broadside: 1,
                battery: 2,
                special: 2,
            },
        );
        assert_eq!(fit.shield, base.shield);
    }

    /// A selection left over from a larger catalogue must still fly a ship
    /// rather than spawning a hull with no guns.
    #[test]
    fn a_stale_selection_falls_back_to_the_first_option() {
        let fit = LoadoutCatalogue::default().fit(
            ShipLoadout::player(),
            SelectedLoadout {
                broadside: 99,
                battery: 99,
                special: 99,
            },
        );
        assert_eq!(fit.broadside.cooldown, 10.0, "the long nines");
        assert!(fit.emp.is_some(), "the disruptor");
        assert!(fit.torpedoes.is_some());
    }
}
