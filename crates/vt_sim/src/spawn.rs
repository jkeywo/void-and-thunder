//! The spawn director and encounter state — the M3 milestone.
//!
//! The director sends escalating waves of hostile ships at the protagonist.
//! When a wave is cleared the next (larger) one arrives; clear them all to win.
//! If the protagonist dies, the encounter is lost. All ship construction lives
//! in [`ship_bundle`] so there is exactly one way to make a ship.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_transform::components::Transform;
use std::f32::consts::TAU;

use crate::combat::Broadside;
use crate::components::{
    AiController, Brace, Collider, EmpDefense, Faction, FireOrders, Heading, Helm, Hull,
    PilotIntent, Protagonist, Ship, ShipStats, SpeedScale, Velocity,
};
use crate::drive::{BoostDrive, MicrowarpDrive};
use crate::emp::EmpWeapon;
use crate::torpedo::{TorpedoBay, TorpedoLaunchQueue, TorpedoLock};
use crate::world::SystemBounds;

/// A ship's full weapon/drive loadout. Every ship carries the same kit; presets
/// differ only in tuning, so a player and an AI ship are the same entity shape.
#[derive(Clone, Copy, Debug)]
pub struct ShipLoadout {
    pub broadside: Broadside,
    pub emp: EmpWeapon,
    pub torpedoes: TorpedoBay,
    pub boost: BoostDrive,
    pub microwarp: MicrowarpDrive,
}

impl Default for ShipLoadout {
    fn default() -> Self {
        Self {
            broadside: Broadside::default(),
            emp: EmpWeapon::default(),
            torpedoes: TorpedoBay::default(),
            boost: BoostDrive::default(),
            microwarp: MicrowarpDrive::default(),
        }
    }
}

impl ShipLoadout {
    /// The player's loadout: a heavy 10s broadside and a 20s microwarp.
    pub fn player() -> Self {
        Self {
            broadside: Broadside {
                cooldown: 10.0,
                ..Broadside::default()
            },
            microwarp: MicrowarpDrive {
                cooldown: 20.0,
                ..MicrowarpDrive::default()
            },
            ..Self::default()
        }
    }

    /// An enemy's loadout: a slower broadside that telegraphs a 0.5s charge.
    pub fn enemy() -> Self {
        Self {
            broadside: Broadside {
                cooldown: 2.8,
                charge_time: 0.5,
                ..Broadside::default()
            },
            ..Self::default()
        }
    }
}

/// The one true ship constructor. Every ship is the same entity shape: base
/// hull + the full [`ShipLoadout`] + a [`PilotIntent`] its controller writes.
/// The player adds [`Protagonist`]; an AI adds [`AiController`]. Which one drives
/// the ship — the client or the sim's AI — is the *only* difference.
pub fn ship_bundle(
    faction: Faction,
    stats: ShipStats,
    hull_max: f32,
    pos: Vec2,
    loadout: ShipLoadout,
) -> impl Bundle {
    (
        Ship,
        faction,
        stats,
        Heading(0.0),
        Velocity::default(),
        Helm::default(),
        FireOrders::default(),
        PilotIntent::default(),
        Brace::default(),
        Hull::new(hull_max),
        Collider::default(),
        EmpDefense::default(),
        SpeedScale::default(),
        Transform::from_translation(pos.extend(0.0)),
        // The full kit — identical set on every ship. Torpedoes carry three
        // components: static config/reload (from the loadout) plus the two
        // always-empty-at-spawn transient lifecycles (aim-lock, launch-queue).
        (
            loadout.broadside,
            loadout.emp,
            loadout.torpedoes,
            TorpedoLock::default(),
            TorpedoLaunchQueue::default(),
            loadout.boost,
            loadout.microwarp,
        ),
    )
}

/// How the current encounter is going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    /// Waves are still coming or enemies are still alive.
    #[default]
    InProgress,
    /// Every wave was cleared — the player won.
    Cleared,
    /// The protagonist was destroyed — the player lost.
    PlayerDestroyed,
}

/// Live state of the encounter, for the HUD and win/lose logic.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct Encounter {
    pub wave: u32,
    pub enemies_remaining: u32,
    pub outcome: Outcome,
}

/// Drives wave spawning.
#[derive(Resource, Clone, Copy, Debug)]
pub struct SpawnDirector {
    /// The last wave number spawned (0 before the first).
    pub wave: u32,
    /// Clear this many waves to win.
    pub max_waves: u32,
    /// Ships in wave 1; each later wave adds one.
    pub base_count: u32,
    /// Faction of the ships that are sent.
    pub faction: Faction,
    /// Hull each spawned ship starts with (rises with the wave).
    pub base_hull: f32,
    /// RNG state for jittering spawn angles.
    seed: u32,
}

impl Default for SpawnDirector {
    fn default() -> Self {
        Self {
            wave: 0,
            max_waves: 3,
            base_count: 2,
            faction: Faction::Houses,
            base_hull: 100.0,
            seed: 0x1234_5678,
        }
    }
}

impl SpawnDirector {
    fn next_seed(&mut self) -> f32 {
        crate::util::lcg_next(&mut self.seed)
    }
}

/// Number of ships in a given wave (1-based).
pub fn wave_size(wave: u32, base_count: u32) -> u32 {
    base_count + wave.saturating_sub(1)
}

/// Positions for a wave: a ring near the system edge, evenly spread with a
/// per-wave angular jitter so waves don't arrive from identical bearings.
pub fn wave_spawn_points(count: u32, radius: f32, jitter: f32) -> Vec<Vec2> {
    let count = count.max(1);
    (0..count)
        .map(|i| {
            let angle = (i as f32 / count as f32) * TAU + jitter * TAU;
            Vec2::from_angle(angle) * radius
        })
        .collect()
}

/// Bevy system: run the encounter — spawn waves, detect win/lose.
pub fn director_system(
    mut commands: Commands,
    mut director: ResMut<SpawnDirector>,
    mut encounter: ResMut<Encounter>,
    bounds: Res<SystemBounds>,
    protagonist: Query<(), With<Protagonist>>,
    // Exclude the protagonist: it may itself be AI-piloted (player-AI toggle).
    enemies: Query<(), (With<Ship>, With<AiController>, Without<Protagonist>)>,
) {
    if encounter.outcome != Outcome::InProgress {
        return;
    }

    // Lose: the protagonist is gone.
    if protagonist.is_empty() {
        encounter.outcome = Outcome::PlayerDestroyed;
        return;
    }

    let alive = enemies.iter().count() as u32;
    encounter.enemies_remaining = alive;
    if alive > 0 {
        return; // fight the current wave
    }

    // Current wave cleared — win, or send the next.
    if director.wave >= director.max_waves {
        encounter.outcome = Outcome::Cleared;
        return;
    }

    director.wave += 1;
    encounter.wave = director.wave;

    let count = wave_size(director.wave, director.base_count);
    let hull = director.base_hull + (director.wave - 1) as f32 * 25.0;
    let jitter = director.next_seed();
    let stats = ShipStats {
        thrust: 660.0,
        turn_rate: 0.35,
        max_speed: 100.0,
        ..Default::default()
    };
    for pos in wave_spawn_points(count, bounds.radius * 0.85, jitter) {
        commands.spawn((
            ship_bundle(director.faction, stats, hull, pos, ShipLoadout::enemy()),
            AiController::default(),
        ));
    }
    encounter.enemies_remaining = count;
}

/// Reset the encounter to its opening state. The client calls this on restart
/// (after despawning the old ships) to begin a fresh run.
pub fn reset_encounter(director: &mut SpawnDirector, encounter: &mut Encounter) {
    let seed = director.seed;
    *director = SpawnDirector {
        seed,
        ..Default::default()
    };
    *encounter = Encounter::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_escalate_in_size() {
        assert_eq!(wave_size(1, 2), 2);
        assert_eq!(wave_size(2, 2), 3);
        assert_eq!(wave_size(3, 2), 4);
    }

    #[test]
    fn spawn_points_ring_the_edge() {
        let pts = wave_spawn_points(4, 1000.0, 0.0);
        assert_eq!(pts.len(), 4);
        for p in pts {
            assert!(
                (p.length() - 1000.0).abs() < 1e-2,
                "point off the ring: {p:?}"
            );
        }
    }

    #[test]
    fn director_spawns_the_first_wave() {
        let mut world = World::new();
        world.insert_resource(SpawnDirector::default());
        world.insert_resource(Encounter::default());
        world.insert_resource(SystemBounds::default());
        world.spawn(Protagonist);

        let mut schedule = Schedule::default();
        schedule.add_systems(director_system);
        schedule.run(&mut world);

        let enemies = world
            .query_filtered::<(), (With<Ship>, With<AiController>)>()
            .iter(&world)
            .count();
        assert_eq!(enemies, 2, "wave 1 should spawn base_count ships");
        assert_eq!(world.resource::<Encounter>().wave, 1);
    }

    #[test]
    fn losing_the_protagonist_ends_the_encounter() {
        let mut world = World::new();
        world.insert_resource(SpawnDirector::default());
        world.insert_resource(Encounter::default());
        world.insert_resource(SystemBounds::default());
        // No Protagonist entity at all.

        let mut schedule = Schedule::default();
        schedule.add_systems(director_system);
        schedule.run(&mut world);

        assert_eq!(
            world.resource::<Encounter>().outcome,
            Outcome::PlayerDestroyed
        );
    }
}
