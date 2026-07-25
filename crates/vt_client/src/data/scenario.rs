//! Scenarios: an encounter as data.
//!
//! A scenario says where the player starts, what is already out there, and
//! whether a wave director runs on top. That covers both the game's normal
//! encounter and a bare test range, so there is one spawn path rather than a
//! normal path plus a special case for the range.
//!
//! `director: None` is the load-bearing bit: it suppresses waves entirely, which
//! is what lets a scenario place one target and have the encounter be exactly
//! that.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use vt_sim::prelude::{ship_bundle, Anchored, Faction, Invulnerable, Protagonist};

use crate::data::ships::{DirectorSpec, ShipTable};
use crate::Player;

/// Per-ship overrides a scenario can apply. All default to off, so a placed ship
/// is an ordinary ship unless the file says otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShipFlags {
    /// Takes no hull damage. Hits still register — sparks, sound and shake all
    /// fire — so you can see your shots land on something that never dies.
    pub invulnerable: bool,
    /// No controller at all: it never steers, never fires, and never counts as
    /// an enemy the encounter is waiting on. Needs no component — it is simply
    /// the absence of an `AiController`.
    pub inert: bool,
    /// Never moves, whatever writes its helm.
    pub anchored: bool,
}

/// One ship placed by a scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacedShip {
    /// Name of its [`ShipClass`](crate::data::ships::ShipClass).
    pub class: String,
    pub faction: Faction,
    pub pos: Vec2,
    /// Facing, in radians. `0` points along +X.
    pub heading: f32,
    /// Starting hull, or the class's own if omitted.
    pub hull: Option<f32>,
    pub flags: ShipFlags,
}

impl Default for PlacedShip {
    fn default() -> Self {
        Self {
            class: "house_patrol".into(),
            faction: Faction::Houses,
            pos: Vec2::ZERO,
            heading: 0.0,
            hull: None,
            flags: ShipFlags::default(),
        }
    }
}

/// An authored encounter.
#[derive(Asset, TypePath, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scenario {
    /// Shown on the title card and in the design panel's scenario list.
    pub name: String,
    /// Radius of the playfield disc.
    pub bounds_radius: f32,
    pub player: PlacedShip,
    /// Ships already in the system when the run begins.
    pub enemies: Vec<PlacedShip>,
    /// Waves on top of the placed ships, or `None` for no waves at all.
    pub director: Option<DirectorSpec>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            name: "Skirmish".into(),
            bounds_radius: 1400.0,
            player: PlacedShip {
                class: "corsair_sloop".into(),
                faction: Faction::Corsairs,
                pos: Vec2::new(0.0, -520.0),
                heading: std::f32::consts::FRAC_PI_2,
                hull: None,
                flags: ShipFlags::default(),
            },
            enemies: Vec::new(),
            director: Some(DirectorSpec::default()),
        }
    }
}

/// Spawn one placed ship. `protagonist` marks it as the ship the encounter
/// revolves around and routes the client's input into it.
///
/// Returns `false` when the class name is unknown, so the caller can say which
/// scenario is wrong rather than quietly flying the wrong ship.
pub fn spawn_placed(
    commands: &mut Commands,
    table: &ShipTable,
    placed: &PlacedShip,
    protagonist: bool,
) -> bool {
    let Some((class_id, class)) = table.find(&placed.class) else {
        return false;
    };

    let mut entity = commands.spawn(ship_bundle(
        placed.faction,
        class.stats,
        placed.hull.unwrap_or(class.hull),
        placed.pos,
        placed.heading,
        class.loadout,
    ));

    // Inserted *after* the bundle, not alongside it: `ship_bundle` already
    // carries a default `Collider` and `EmpDefense`, and Bevy panics on a bundle
    // that names the same component twice. A later insert overwrites instead.
    entity.insert((class.collider, class.emp_defense, class_id));

    if protagonist {
        entity.insert((Player, Protagonist));
    } else if !placed.flags.inert {
        // Inert means "no controller", which is simply not inserting one. That
        // also keeps it out of the director's live-enemy count, so a test-range
        // target never leaves the encounter waiting on it.
        entity.insert(class.ai);
    }

    if placed.flags.invulnerable {
        entity.insert(Invulnerable);
    }
    if placed.flags.anchored {
        entity.insert(Anchored);
    }
    true
}

/// Spawn everything a scenario places: the player, then its standing ships.
///
/// Logs each unknown class rather than failing the whole scenario — losing one
/// ship out of a range is recoverable; losing the player is what the caller
/// notices anyway, since the director immediately calls the run lost.
pub fn spawn_scenario(commands: &mut Commands, table: &ShipTable, scenario: &Scenario) {
    if !spawn_placed(commands, table, &scenario.player, true) {
        error!(
            "scenario '{}': unknown player class '{}' — no ship spawned",
            scenario.name, scenario.player.class
        );
    }
    for placed in &scenario.enemies {
        if !spawn_placed(commands, table, placed, false) {
            error!(
                "scenario '{}': unknown class '{}' — ship skipped",
                scenario.name, placed.class
            );
        }
    }
}

/// Resolve a scenario's director spec against the class table.
pub fn director_for(
    scenario: &Scenario,
    table: &ShipTable,
) -> Option<vt_sim::prelude::DirectorSettings> {
    let spec = scenario.director.as_ref()?;
    let resolved = spec.resolve(table);
    if resolved.is_none() {
        error!(
            "scenario '{}': director names unknown class '{}' — running with no waves",
            scenario.name, spec.enemy_class
        );
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default scenario must reproduce the encounter the game shipped with,
    /// so a missing `skirmish.scn.ron` still plays the normal game.
    #[test]
    fn the_default_scenario_is_the_original_encounter() {
        let s = Scenario::default();
        assert_eq!(s.player.pos, Vec2::new(0.0, -520.0));
        assert_eq!(s.player.class, "corsair_sloop");
        assert!(s.enemies.is_empty(), "waves supply the enemies");
        let director = s.director.expect("the skirmish has waves");
        assert_eq!(director.max_waves, 3);
        assert_eq!(director.base_count, 2);
    }

    /// A scenario with no director resolves to no waves — the test range.
    #[test]
    fn a_scenario_without_a_director_yields_no_waves() {
        let scenario = Scenario {
            director: None,
            ..Scenario::default()
        };
        assert!(director_for(&scenario, &ShipTable::default()).is_none());
    }

    /// Flags are off unless asked for, so an ordinary placed ship stays ordinary.
    #[test]
    fn placed_ships_are_ordinary_by_default() {
        let placed = PlacedShip::default();
        assert!(!placed.flags.invulnerable);
        assert!(!placed.flags.inert);
        assert!(!placed.flags.anchored);
        assert!(placed.hull.is_none(), "hull comes from the class");
    }

    /// Run `spawn_scenario` against a real `World` and check what actually
    /// landed on the entities. The flags are only worth anything if they reach
    /// the ECS, and the first version of this spawned a bundle with duplicate
    /// `Collider`/`EmpDefense` components — which Bevy panics on, and which no
    /// amount of parsing the RON would have caught.
    fn spawn_into_world(scenario: &Scenario) -> World {
        let mut world = World::new();
        let table = ShipTable::default();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            spawn_scenario(&mut commands, &table, scenario);
        }
        queue.apply(&mut world);
        world
    }

    #[test]
    fn the_test_range_spawns_one_inert_invulnerable_anchored_target() {
        let text = include_str!("../../assets/data/scenarios/test_range.scn.ron");
        let scenario: Scenario = ron::from_str(text).expect("test_range.scn.ron parses");
        let mut world = spawn_into_world(&scenario);

        let players = world
            .query_filtered::<(), (With<Player>, With<Protagonist>)>()
            .iter(&world)
            .count();
        assert_eq!(players, 1, "the range spawns exactly one player ship");

        let targets: Vec<Entity> = world
            .query_filtered::<Entity, (With<vt_sim::prelude::Ship>, Without<Protagonist>)>()
            .iter(&world)
            .collect();
        assert_eq!(targets.len(), 1, "the range spawns exactly one target");

        let target = targets[0];
        assert!(
            world.get::<Invulnerable>(target).is_some(),
            "the target must survive being shot"
        );
        assert!(
            world.get::<Anchored>(target).is_some(),
            "the target must hold its mark"
        );
        assert!(
            world.get::<vt_sim::prelude::AiController>(target).is_none(),
            "inert means no controller at all — that is also what keeps it out \
             of the director's live-enemy count"
        );
    }

    /// The ordinary encounter's player must be a normal ship: none of the range
    /// flags leaking into it.
    #[test]
    fn the_skirmish_player_carries_no_range_flags() {
        let mut world = spawn_into_world(&Scenario::default());
        let player = world
            .query_filtered::<Entity, With<Protagonist>>()
            .iter(&world)
            .next()
            .expect("a player");
        assert!(world.get::<Invulnerable>(player).is_none());
        assert!(world.get::<Anchored>(player).is_none());
    }
}
