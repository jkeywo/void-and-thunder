//! Ship classes: the authored description of what a ship *is*.
//!
//! A class is everything `ship_bundle` needs, under a name. Ships are then
//! instances of a class rather than hand-written struct literals, which is what
//! lets the design panel edit "the corsair sloop" once and have every corsair
//! sloop in the world follow.
//!
//! Classes are an ordered list, not a map, because a ship carries its class as a
//! [`ClassId`] index. That keeps the sim string-free and `Copy`. The cost is that
//! reordering the list while the game is running reassigns live ships to their
//! new neighbours — harmless in a dev tool, and a restart puts it right.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use vt_sim::prelude::{
    AiController, ClassId, Collider, EmpDefense, ShipLoadout, ShipStats, SpawnDirector,
};

/// One authored ship class.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShipClass {
    /// Handling: thrust, turn rate, top speed, drag.
    pub stats: ShipStats,
    /// Hull the ship spawns with (also its maximum).
    pub hull: f32,
    /// Collision radius for hit tests.
    pub collider: Collider,
    /// EMP resistance and recovery.
    pub emp_defense: EmpDefense,
    /// Weapons and drives.
    pub loadout: ShipLoadout,
    /// AI tuning for ships of this class that steer themselves. A placed ship
    /// can still be spawned inert (no controller at all) regardless.
    pub ai: AiController,
}

impl Default for ShipClass {
    fn default() -> Self {
        Self {
            stats: ShipStats::default(),
            hull: 100.0,
            collider: Collider::default(),
            emp_defense: EmpDefense::default(),
            loadout: ShipLoadout::default(),
            ai: AiController::default(),
        }
    }
}

/// A named entry in the class table. The name lives beside the class rather than
/// keying a map so the file keeps a stable, readable order.
///
/// The class is nested rather than `#[serde(flatten)]`ed: flatten needs
/// map-based deserialization, which RON's struct syntax does not offer, and the
/// failure is an unhelpful `ExpectedMap` pointing at the first nested field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedClass {
    pub name: String,
    pub class: ShipClass,
}

impl Default for NamedClass {
    fn default() -> Self {
        Self {
            name: "unnamed".into(),
            class: ShipClass::default(),
        }
    }
}

/// Every ship class the game knows about.
#[derive(Asset, Resource, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShipTable {
    pub classes: Vec<NamedClass>,
}

impl Default for ShipTable {
    /// The two classes the game shipped with before classes existed, so a missing
    /// or unreadable `ships.ron` still yields a playable encounter.
    fn default() -> Self {
        Self {
            classes: vec![
                NamedClass {
                    name: "corsair_sloop".into(),
                    class: ShipClass {
                        // Half the stock hull: the player is the fragile one, and
                        // the shields are what keep them alive.
                        hull: 50.0,
                        loadout: ShipLoadout::player(),
                        ..ShipClass::default()
                    },
                },
                NamedClass {
                    name: "house_patrol".into(),
                    class: ShipClass {
                        stats: ShipStats {
                            thrust: 98.0,
                            turn_rate: 0.35,
                            max_speed: 100.0,
                            forward_drag: 0.95,
                            lateral_drag: 5.0,
                            turn_rate_slow: 1.35,
                            turn_rate_fast: 0.5,
                            turn_accel: 3.2,
                        },
                        loadout: ShipLoadout::enemy(),
                        ..ShipClass::default()
                    },
                },
                NamedClass {
                    name: "house_bastion".into(),
                    class: ShipClass {
                        // A siege platform: barely mobile, three times a
                        // patrol's hull, shielded all round, guns all round.
                        stats: ShipStats {
                            thrust: 30.0,
                            turn_rate: 0.16,
                            max_speed: 34.0,
                            forward_drag: 0.9,
                            lateral_drag: 6.0,
                            turn_rate_slow: 1.2,
                            turn_rate_fast: 0.7,
                            turn_accel: 1.4,
                        },
                        hull: 300.0,
                        collider: Collider { radius: 42.0 },
                        emp_defense: EmpDefense {
                            resist: 220.0,
                            recovery_per_sec: 20.0,
                            ..EmpDefense::default()
                        },
                        loadout: ShipLoadout::bastion(),
                        ai: AiController {
                            engage_range: 520.0,
                            fire_arc: std::f32::consts::PI,
                            aim_at_target: true,
                            flee_hull_frac: 0.0,
                            use_abilities: false,
                        },
                    },
                },
            ],
        }
    }
}

impl ShipTable {
    /// Look a class up by name, with its index.
    ///
    /// Returns `None` for an unknown name rather than substituting something —
    /// a scenario naming a class that doesn't exist is an authoring mistake, and
    /// silently flying a different ship would hide it.
    pub fn find(&self, name: &str) -> Option<(ClassId, &ShipClass)> {
        self.classes
            .iter()
            .position(|c| c.name == name)
            .map(|i| (ClassId(i as u16), &self.classes[i].class))
    }

    /// The class a [`ClassId`] refers to, if it is still in range.
    ///
    /// The next three are the design panel's read/write surface. They are
    /// `dead_code` until that panel exists, which is a truthful thing for the
    /// compiler to say — the allow is scoped to them rather than the module so
    /// it stops being needed the moment the panel lands.
    #[allow(dead_code)]
    pub fn get(&self, id: ClassId) -> Option<&ShipClass> {
        self.classes.get(id.0 as usize).map(|c| &c.class)
    }

    /// Mutable access, for the design panel.
    #[allow(dead_code)]
    pub fn get_mut(&mut self, id: ClassId) -> Option<&mut ShipClass> {
        self.classes.get_mut(id.0 as usize).map(|c| &mut c.class)
    }

    /// Names in table order — what the panel's class list shows.
    #[allow(dead_code)]
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.classes.iter().map(|c| c.name.as_str())
    }
}

/// How a scenario asks for waves. The authored half: it names a class, where the
/// sim's [`DirectorSettings`](vt_sim::prelude::DirectorSettings) carries the
/// resolved copy, so "what an enemy ship is" is described in exactly one place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectorSpec {
    pub max_waves: u32,
    pub base_count: u32,
    pub faction: vt_sim::prelude::Faction,
    pub base_hull: f32,
    pub hull_per_wave: f32,
    /// Name of the class the waves are made of.
    pub enemy_class: String,
    /// Name of the class that *replaces* the last wave, if any. An unknown or
    /// empty name simply means the run ends on one more ordinary wave.
    pub finale_class: String,
    /// How many of it to send.
    pub finale_count: u32,
}

impl Default for DirectorSpec {
    fn default() -> Self {
        Self {
            max_waves: 3,
            base_count: 2,
            faction: vt_sim::prelude::Faction::Houses,
            base_hull: 100.0,
            hull_per_wave: 25.0,
            enemy_class: "house_patrol".into(),
            finale_class: "house_bastion".into(),
            finale_count: 1,
        }
    }
}

impl DirectorSpec {
    /// Resolve into the sim's settings, or `None` if the named class is missing.
    pub fn resolve(&self, table: &ShipTable) -> Option<vt_sim::prelude::DirectorSettings> {
        let (class_id, class) = table.find(&self.enemy_class)?;
        // A finale is optional and *silently* optional: an empty name is the
        // normal case, and a name that resolves to nothing leaves the run
        // ending on an ordinary wave rather than failing to start.
        let finale = table
            .find(&self.finale_class)
            .map(|(id, boss)| vt_sim::prelude::FinaleWave {
                count: self.finale_count.max(1),
                hull: boss.hull,
                stats: boss.stats,
                loadout: boss.loadout,
                class: id,
                ai: boss.ai,
            });
        Some(vt_sim::prelude::DirectorSettings {
            max_waves: self.max_waves,
            base_count: self.base_count,
            faction: self.faction,
            base_hull: self.base_hull,
            hull_per_wave: self.hull_per_wave,
            stats: class.stats,
            loadout: class.loadout,
            class: class_id,
            finale,
        })
    }
}

/// Set the director from a scenario's spec, preserving the RNG seed so a restart
/// replays the same waves.
pub fn set_director(
    director: &mut SpawnDirector,
    settings: Option<vt_sim::prelude::DirectorSettings>,
) {
    director.wave = 0;
    director.settings = settings;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fallback table must reproduce the loadouts the game shipped with, so
    /// a missing `ships.ron` plays identically rather than subtly differently.
    #[test]
    fn the_default_table_matches_the_original_loadouts() {
        let table = ShipTable::default();
        let (_, player) = table.find("corsair_sloop").expect("player class");
        let (_, enemy) = table.find("house_patrol").expect("enemy class");
        assert_eq!(player.loadout, ShipLoadout::player());
        assert_eq!(enemy.loadout, ShipLoadout::enemy());
    }

    /// The compiled-in fallback and the shipped file must describe the *same*
    /// ships. Asserting against the file rather than against copied literals is
    /// what stops the two drifting the next time a hull is retuned — with
    /// literals, only the one you remembered to edit moves.
    #[test]
    fn the_default_table_matches_the_shipped_file() {
        let text = include_str!("../../assets/data/ships.ron");
        let shipped: ShipTable = ron::from_str(text).expect("ships.ron should parse");
        assert_eq!(
            shipped,
            ShipTable::default(),
            "assets/data/ships.ron has drifted from ShipTable::default()"
        );
    }

    #[test]
    fn an_unknown_class_resolves_to_nothing() {
        let table = ShipTable::default();
        assert!(table.find("dreadnought").is_none());
        let spec = DirectorSpec {
            enemy_class: "dreadnought".into(),
            ..DirectorSpec::default()
        };
        assert!(spec.resolve(&table).is_none());
    }

    #[test]
    fn a_resolved_director_carries_the_class_it_came_from() {
        let table = ShipTable::default();
        let settings = DirectorSpec::default().resolve(&table).expect("resolves");
        let (id, class) = table.find("house_patrol").unwrap();
        assert_eq!(settings.class, id);
        assert_eq!(settings.stats, class.stats);
        assert_eq!(settings.loadout, class.loadout);
    }
}
