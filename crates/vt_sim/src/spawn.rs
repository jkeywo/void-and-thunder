//! The spawn director and encounter state — the M3 milestone.
//!
//! The director sends escalating waves of hostile ships at the protagonist.
//! When a wave is cleared the next (larger) one arrives; clear them all to win.
//! If the protagonist dies, the encounter is lost. All ship construction lives
//! in [`ship_bundle`] so there is exactly one way to make a ship.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec2};
use bevy_reflect::Reflect;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

use crate::combat::Broadside;
use crate::components::{
    AiController, AngularVelocity, Brace, ClassId, Collider, EmpDefense, Faction, FireOrders,
    Heading, Helm, Hull, PilotIntent, Protagonist, Ship, ShipStats, SpeedScale, Velocity,
};
use crate::drive::{BoostDrive, MicrowarpDrive};
use crate::emp::EmpWeapon;
use crate::pilot::PilotBrain;
use crate::shield::Shield;
use crate::torpedo::{TorpedoBay, TorpedoLaunchQueue, TorpedoLock};
use crate::world::SystemBounds;

/// A ship's full weapon/drive loadout. Every ship carries the same kit; presets
/// differ only in tuning, so a player and an AI ship are the same entity shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct ShipLoadout {
    pub broadside: Broadside,
    pub emp: EmpWeapon,
    pub torpedoes: TorpedoBay,
    pub boost: BoostDrive,
    pub microwarp: MicrowarpDrive,
    /// Fore/aft shields. Defaults to a `max` of zero — no shields fitted — so a
    /// class opts in with one authored number and every existing hull is
    /// unchanged.
    pub shield: Shield,
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
            // Fore and aft banks of 22.5 against a 50-point hull: a
            // well-presented side still roughly doubles your effective health,
            // but every exchange matters more than it did. Enemies fit none
            // (see `enemy`), which is a per-class decision rather than a rule.
            shield: Shield {
                fore_max: 22.5,
                aft_max: 22.5,
                ..Shield::default()
            },
            ..Self::default()
        }
    }

    /// The bastion's loadout: guns that bear in every direction on a short
    /// cooldown, behind a single pooled shield that covers every direction too.
    pub fn bastion() -> Self {
        Self {
            broadside: Broadside {
                cooldown: 1.0,
                damage: 5.0,
                muzzle_speed: 300.0,
                guns: 2,
                // A *half*-width, so pi is the whole circle.
                arc: std::f32::consts::PI,
                charge_time: 0.0,
                ..Broadside::default()
            },
            shield: Shield {
                // One pool over the whole hull. Two equal banks would still have
                // rewarded circling — flatten the side facing you, swing round,
                // find the other one full — and the point is that there is no
                // side of this thing worth manoeuvring for.
                pooled: true,
                fore_max: 120.0,
                aft_max: 0.0,
                regen_per_sec: 9.0,
                regen_delay: 3.0,
                ..Shield::default()
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
/// hull + the full [`ShipLoadout`] + a [`PilotIntent`] its controller writes
/// (alongside the [`PilotBrain`] the utility AI thinks in, empty until an
/// [`AiController`] with the kit enabled takes the ship over).
/// The player adds [`Protagonist`]; an AI adds [`AiController`]. Which one drives
/// the ship — the client or the sim's AI — is the *only* difference.
///
/// `heading` seeds both the sim's [`Heading`] and the `Transform` rotation.
/// Movement rewrites the rotation every step for a ship that moves, but a ship
/// that never moves (an anchored target) would otherwise render bow-along-+X.
pub fn ship_bundle(
    faction: Faction,
    stats: ShipStats,
    hull_max: f32,
    pos: Vec2,
    heading: f32,
    loadout: ShipLoadout,
) -> impl Bundle {
    (
        Ship,
        faction,
        stats,
        // Motion state, nested to stay inside Bevy's 15-element tuple limit.
        (
            Heading(heading),
            Velocity::default(),
            AngularVelocity::default(),
        ),
        Helm::default(),
        FireOrders::default(),
        (PilotIntent::default(), PilotBrain::default()),
        Brace::default(),
        Hull::new(hull_max),
        // `charged` is what turns the authored *fit* into live state: the file
        // says how big the banks are, and a ship always enters the field with
        // them full.
        loadout.shield.charged(),
        Collider::default(),
        EmpDefense::default(),
        SpeedScale::default(),
        Transform::from_translation(pos.extend(0.0)).with_rotation(Quat::from_rotation_z(heading)),
        // The full kit — identical set on every ship. Torpedoes carry three
        // components: static config/reload (from the loadout) plus the two
        // always-empty-at-spawn transient lifecycles (aim-lock, launch-queue).
        (
            loadout.broadside,
            loadout.emp,
            loadout.torpedoes.stocked(),
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

/// A different ship for the last wave — the thing the run has been building
/// toward. Optional, because a scenario is perfectly entitled to end on one more
/// wave of the same.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FinaleWave {
    /// How many of it. Usually one: the point is a single hard problem, not a
    /// bigger crowd.
    pub count: u32,
    pub hull: f32,
    pub stats: ShipStats,
    pub loadout: ShipLoadout,
    pub class: ClassId,
    pub ai: AiController,
}

impl Default for FinaleWave {
    fn default() -> Self {
        Self {
            count: 1,
            hull: 300.0,
            stats: ShipStats::default(),
            loadout: ShipLoadout::default(),
            class: ClassId(0),
            ai: AiController::default(),
        }
    }
}

/// How an encounter's waves are shaped. This is the *authored* half of the
/// director — everything a scenario file gets to say about the waves. The live
/// half (which wave we're on, the RNG state) lives on [`SpawnDirector`], so this
/// can be serialised without dragging mid-run state into the file.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectorSettings {
    /// Clear this many waves to win.
    pub max_waves: u32,
    /// Ships in wave 1; each later wave adds one.
    pub base_count: u32,
    /// Faction of the ships that are sent.
    pub faction: Faction,
    /// Hull the first wave's ships start with.
    pub base_hull: f32,
    /// Extra hull each wave adds on top of `base_hull`.
    pub hull_per_wave: f32,
    /// Handling of the ships that are sent. Resolved from the scenario's named
    /// ship class, so waves and placed ships describe a ship exactly one way.
    pub stats: ShipStats,
    /// Weapons and drives of the ships that are sent, likewise resolved.
    pub loadout: ShipLoadout,
    /// Which class the above came from, stamped onto each spawned ship so a
    /// class edited in the design panel reaches wave ships too.
    pub class: ClassId,
    /// Replaces the *last* wave, when a scenario names one.
    pub finale: Option<FinaleWave>,
}

impl Default for DirectorSettings {
    fn default() -> Self {
        Self {
            max_waves: 3,
            base_count: 2,
            faction: Faction::Houses,
            base_hull: 100.0,
            hull_per_wave: 25.0,
            // A House patrol: heavier and slower than the player's sloop.
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
            class: ClassId(0),
            finale: None,
        }
    }
}

/// Drives wave spawning. `settings` of `None` suppresses waves entirely — an
/// authored scenario (the test range) that places its own ships and wants no
/// director on top.
#[derive(Resource, Clone, Copy, Debug)]
pub struct SpawnDirector {
    /// The last wave number spawned (0 before the first).
    pub wave: u32,
    /// How the waves are shaped, or `None` for no waves at all.
    pub settings: Option<DirectorSettings>,
    /// RNG state for jittering spawn angles.
    seed: u32,
}

impl Default for SpawnDirector {
    fn default() -> Self {
        Self {
            wave: 0,
            settings: Some(DirectorSettings::default()),
            seed: 0x1234_5678,
        }
    }
}

impl SpawnDirector {
    /// A director that sends no waves — the encounter is whatever was placed.
    pub fn silent() -> Self {
        Self {
            settings: None,
            ..Self::default()
        }
    }

    /// A director with authored waves and a caller-chosen jitter seed — the
    /// corpus seam: the same scenario driven under different seeds spawns its
    /// waves at different angles, which is what makes a batch of runs a
    /// measurement rather than one run repeated.
    pub fn seeded(settings: Option<DirectorSettings>, seed: u32) -> Self {
        Self {
            wave: 0,
            settings,
            seed,
        }
    }

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

    // Lose: the protagonist is gone. Checked before the no-waves branch below, so
    // an authored scenario can still be *lost* even though it can never be won by
    // clearing waves it doesn't have.
    if protagonist.is_empty() {
        encounter.outcome = Outcome::PlayerDestroyed;
        return;
    }

    // No director: whatever the scenario placed is the whole encounter. Report
    // what's alive and stop — never "Cleared", since an inert target carries no
    // `AiController` and so is never counted as an enemy to clear.
    let Some(settings) = director.settings else {
        encounter.enemies_remaining = enemies.iter().count() as u32;
        return;
    };

    let alive = enemies.iter().count() as u32;
    encounter.enemies_remaining = alive;
    if alive > 0 {
        return; // fight the current wave
    }

    // Current wave cleared — win, or send the next.
    if director.wave >= settings.max_waves {
        encounter.outcome = Outcome::Cleared;
        return;
    }

    director.wave += 1;
    encounter.wave = director.wave;

    // The last wave may be something else entirely.
    let finale = settings
        .finale
        .filter(|_| director.wave == settings.max_waves);
    let (count, hull, stats, loadout, class, ai) = match finale {
        Some(f) => (f.count, f.hull, f.stats, f.loadout, f.class, f.ai),
        None => (
            wave_size(director.wave, settings.base_count),
            settings.base_hull + (director.wave - 1) as f32 * settings.hull_per_wave,
            settings.stats,
            settings.loadout,
            settings.class,
            AiController::default(),
        ),
    };
    let jitter = director.next_seed();
    for pos in wave_spawn_points(count, bounds.radius * 0.85, jitter) {
        // Face the origin — waves ring the system edge, so this points each
        // arrival inward, at the action.
        let heading = (-pos).to_angle();
        commands.spawn((
            ship_bundle(faction_of(&settings), stats, hull, pos, heading, loadout),
            class,
            ai,
        ));
    }
    encounter.enemies_remaining = count;
}

/// The faction the director's ships fly under. Split out so the finale reads
/// the same as any other wave — a boss is still a House ship.
fn faction_of(settings: &DirectorSettings) -> Faction {
    settings.faction
}

/// Reset the encounter to its opening state. The client calls this on restart
/// (after despawning the old ships) to begin a fresh run.
///
/// The RNG seed *and* the authored settings survive, so restarting replays the
/// same scenario — a restarted test range must not suddenly grow waves.
pub fn reset_encounter(director: &mut SpawnDirector, encounter: &mut Encounter) {
    *director = SpawnDirector {
        wave: 0,
        settings: director.settings,
        seed: director.seed,
    };
    *encounter = Encounter::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last wave is the thing the run has been building toward, and it is a
    /// *different* ship — not more of the same, and not scaled by the ordinary
    /// hull-per-wave ramp.
    #[test]
    fn the_finale_replaces_the_last_wave() {
        let mut world = World::new();
        world.init_resource::<Encounter>();
        world.insert_resource(SystemBounds::default());
        let boss = ClassId(7);
        world.insert_resource(SpawnDirector {
            settings: Some(DirectorSettings {
                max_waves: 2,
                base_count: 2,
                finale: Some(FinaleWave {
                    count: 1,
                    hull: 300.0,
                    class: boss,
                    ..FinaleWave::default()
                }),
                ..DirectorSettings::default()
            }),
            ..SpawnDirector::default()
        });
        world.spawn((Protagonist, Transform::default()));

        let mut schedule = Schedule::default();
        schedule.add_systems(director_system);

        // Wave 1: ordinary patrols.
        schedule.run(&mut world);
        let wave1: Vec<ClassId> = world
            .query_filtered::<&ClassId, With<AiController>>()
            .iter(&world)
            .copied()
            .collect();
        assert_eq!(wave1.len(), 2, "wave one is the authored base count");
        assert!(wave1.iter().all(|c| *c != boss), "and not the finale");

        // Clear it; the last wave should be the boss, alone.
        let ids: Vec<Entity> = world
            .query_filtered::<Entity, With<AiController>>()
            .iter(&world)
            .collect();
        for e in ids {
            world.despawn(e);
        }
        schedule.run(&mut world);
        let wave2: Vec<(ClassId, f32)> = world
            .query_filtered::<(&ClassId, &Hull), With<AiController>>()
            .iter(&world)
            .map(|(c, h)| (*c, h.max))
            .collect();
        assert_eq!(
            wave2.len(),
            1,
            "the finale sends one hard problem, not a crowd"
        );
        assert_eq!(wave2[0].0, boss, "and it is the finale's class");
        assert_eq!(wave2[0].1, 300.0, "with its own hull, not the wave ramp's");
    }

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

    /// A scenario with no director settings places its own ships and gets no
    /// waves — and must never resolve as `Cleared`, however few enemies the
    /// director can see (an inert target carries no `AiController` at all).
    #[test]
    fn a_silent_director_spawns_nothing_and_never_clears() {
        let mut world = World::new();
        world.insert_resource(SpawnDirector::silent());
        world.insert_resource(Encounter::default());
        world.insert_resource(SystemBounds::default());
        world.spawn(Protagonist);

        let mut schedule = Schedule::default();
        schedule.add_systems(director_system);
        for _ in 0..8 {
            schedule.run(&mut world);
        }

        let ships = world
            .query_filtered::<(), With<Ship>>()
            .iter(&world)
            .count();
        assert_eq!(ships, 0, "a silent director must spawn nothing");
        assert_eq!(
            world.resource::<Encounter>().outcome,
            Outcome::InProgress,
            "an authored scenario is never won by clearing waves"
        );
    }

    /// Losing is still possible without a director — the protagonist check must
    /// come *before* the no-waves early return.
    #[test]
    fn a_silent_director_can_still_lose() {
        let mut world = World::new();
        world.insert_resource(SpawnDirector::silent());
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

    /// A restart must replay the same scenario, not fall back to the wave
    /// encounter — otherwise restarting the test range summons a fleet.
    #[test]
    fn resetting_preserves_the_authored_settings() {
        let mut director = SpawnDirector::silent();
        let mut encounter = Encounter {
            wave: 2,
            enemies_remaining: 3,
            outcome: Outcome::Cleared,
        };
        director.wave = 2;

        reset_encounter(&mut director, &mut encounter);

        assert_eq!(director.wave, 0, "the wave counter restarts");
        assert!(
            director.settings.is_none(),
            "the scenario's settings survive"
        );
        assert_eq!(encounter.outcome, Outcome::InProgress);
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
