//! The scenario corpus: every authored scenario driven headlessly by the AI
//! pilot, batched under a budget, and reported.
//!
//! This is the game's answer to "do the scenarios actually play?" — not on
//! paper, but driven. The corpus pilots the *player's own ship* (uniform
//! entity shape: an `AiController` writes the intent the client otherwise
//! would), steps the [`Harness`] through each authored scenario under several
//! seeds, and classifies what happened. The batch/report machinery is
//! `vellum-corpus`; the records, expectations, and vocabulary are this
//! game's.
//!
//! Two kinds of steady state are deliberately told apart: the Skirmish must
//! keep *observably progressing* until it decides (hull totals moving, waves
//! advancing — the stall guard never trips), while the Test Range exists to
//! never decide (an invulnerable, inert, anchored target), so there the
//! stall guard tripping *is* the pass.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use vellum_corpus::{drive, Budget, Provenance, Report, StallGuard, Tally};
use vellum_perf::{compare, render, worst, Baseline, Profile, Recorder, Unit, Verdict};
use vt_sim::prelude::*;

use super::scenario::{director_for, spawn_scenario, Scenario};
use super::ships::ShipTable;

/// The sim's own fixed tick.
const STEP: f32 = 1.0 / 64.0;
/// Steps per stall-guard observation — one in-game second.
const OBSERVE_EVERY: u32 = 64;
/// In-game seconds without observable change before a case is called stalled.
/// Generous: a wave crossing the system takes a while to reach anybody.
const STALL_SECONDS: u32 = 30;
/// Hard cap per case, in in-game seconds.
const CASE_SECONDS: u32 = 240;

/// What one driven case says about itself.
#[derive(Debug, serde::Serialize)]
struct CaseRecord {
    scenario: String,
    seed: u32,
    outcome: &'static str,
    waves: u32,
    sim_seconds: u32,
    stalled: bool,
    player_alive: bool,
}

/// The corpus summary: outcome counts, the decisive rate, and the perf
/// summaries — a vellum-perf capture riding inside a vellum-corpus report as
/// plain serde data, which is the whole envelope-composition story.
#[derive(Debug, serde::Serialize)]
struct Summary {
    outcomes: Tally<&'static str>,
    decisive_permille: u32,
    perf: std::collections::BTreeMap<String, vellum_perf::MetricSummary>,
}

fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::InProgress => "in-progress",
        Outcome::Cleared => "cleared",
        Outcome::PlayerDestroyed => "player-destroyed",
    }
}

/// The coarse state the stall guard watches. Wave, live-enemy count, total
/// hull points and ship count — anything that always advances (time, shot
/// timers) is excluded, or every stall would look like progress.
fn observe(world: &mut World) -> (u32, u32, u32, usize) {
    let encounter = *world.resource::<Encounter>();
    let mut hull_points = 0u32;
    let mut ships = 0usize;
    for hull in world.query_filtered::<&Hull, With<Ship>>().iter(world) {
        hull_points += hull.current.max(0.0) as u32;
        ships += 1;
    }
    (
        encounter.wave,
        encounter.enemies_remaining,
        hull_points,
        ships,
    )
}

/// Drive one scenario once, AI-piloted, under one seed. The recorder takes
/// one wall-clock sample per simulated second — sim throughput, measured
/// from outside the sim, per the measurement contract.
fn run_case(scenario: &Scenario, seed: u32, recorder: &mut Recorder) -> CaseRecord {
    let table = ShipTable::default();
    let mut h = Harness::new();
    h.world.insert_resource(SystemBounds {
        radius: scenario.bounds_radius,
    });
    h.world
        .insert_resource(SpawnDirector::seeded(director_for(scenario, &table), seed));

    let mut queue = CommandQueue::default();
    {
        let mut commands = Commands::new(&mut queue, &h.world);
        spawn_scenario(&mut commands, &table, scenario);
    }
    queue.apply(&mut h.world);

    // The corpus pilots the player's ship itself — the whole point of the
    // uniform entity shape.
    let protagonist = h
        .world
        .query_filtered::<Entity, With<Protagonist>>()
        .single(&h.world)
        .expect("a scenario spawns exactly one player ship");
    h.world
        .entity_mut(protagonist)
        .insert(AiController::piloting());

    let metric = format!("{}.sim-second-ms", scenario.name);
    let mut stall = StallGuard::new(STALL_SECONDS);
    let mut stalled = false;
    let mut sim_seconds = 0;
    for second in 0..CASE_SECONDS {
        let started = std::time::Instant::now();
        h.run(OBSERVE_EVERY, STEP);
        recorder.sample(
            &metric,
            Unit::Millis,
            started.elapsed().as_secs_f64() * 1000.0,
        );
        sim_seconds = second + 1;
        if h.world.resource::<Encounter>().outcome != Outcome::InProgress {
            break;
        }
        if stall.observe(observe(&mut h.world)) {
            stalled = true;
            break;
        }
    }

    let encounter = *h.world.resource::<Encounter>();
    let player_alive = h.world.get::<Hull>(protagonist).is_some();
    CaseRecord {
        scenario: scenario.name.clone(),
        seed,
        outcome: outcome_label(encounter.outcome),
        waves: encounter.wave,
        sim_seconds,
        stalled,
        player_alive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeds per scenario. Different seeds spawn waves at different angles,
    /// which is what makes the batch a measurement rather than one run
    /// repeated.
    const SEEDS: u64 = 3;

    #[test]
    fn the_authored_scenarios_survive_the_corpus() {
        let authored: Vec<Scenario> = [
            include_str!("../../assets/data/scenarios/skirmish.scn.ron"),
            include_str!("../../assets/data/scenarios/test_range.scn.ron"),
        ]
        .into_iter()
        .map(|text| ron::from_str(text).expect("authored scenarios parse"))
        .collect();

        let mut recorder = Recorder::new();
        let batch = drive(
            0..(authored.len() as u64 * SEEDS),
            Budget::cases(authored.len() as u64 * SEEDS),
            |case| {
                let scenario = &authored[(case / SEEDS) as usize];
                run_case(scenario, (case % SEEDS) as u32, &mut recorder)
            },
        );
        let capture = recorder.finish(
            "scenario-corpus",
            Profile {
                runtime: "headless".into(),
                build: if cfg!(debug_assertions) {
                    "dev".into()
                } else {
                    "release".into()
                },
                ..Profile::default()
            },
        );

        let mut outcomes = Tally::new();
        let mut decisive = 0u64;
        for record in &batch.records {
            outcomes.add(record.outcome);
            if record.outcome != "in-progress" {
                decisive += 1;
            }
            match record.scenario.as_str() {
                // The range never decides: no waves, an unkillable target,
                // and the stall guard tripping on that steady state is the
                // expected exit. The player must still be standing.
                "Test Range" => {
                    assert_eq!(record.outcome, "in-progress", "{record:?}");
                    assert_eq!(record.waves, 0, "no director, no waves: {record:?}");
                    assert!(record.stalled, "the range is a steady state: {record:?}");
                    assert!(record.player_alive, "{record:?}");
                }
                // The skirmish must engage and keep observably progressing
                // until it decides. Either ending is legitimate evidence —
                // a corpus measures, it does not flatter — but a stall is a
                // broken encounter.
                "Skirmish" => {
                    assert!(record.waves >= 1, "the director never spawned: {record:?}");
                    assert!(
                        !record.stalled,
                        "the skirmish stopped progressing without deciding: {record:?}"
                    );
                }
                other => panic!("unexpected scenario name '{other}'"),
            }
        }

        let report = Report {
            title: "void-and-thunder scenario corpus".into(),
            provenance: Provenance {
                runner: "vt_client::data::corpus".into(),
                cases: format!("{} authored scenarios x {SEEDS} seeds", authored.len()),
                fingerprint: None,
                exhausted: batch.exhausted,
                elapsed_seconds: batch.elapsed_seconds,
            },
            summary: Summary {
                decisive_permille: vellum_corpus::permille(decisive, outcomes.total()),
                outcomes,
                perf: capture.summaries.clone(),
            },
            records: batch.records,
        };
        // Visible under `cargo test -- --nocapture`; the shape CI tooling
        // will pick up once corpus reports become artifacts.
        eprintln!("{}", report.to_json());

        // The baseline comparison is warnings-first, exactly as the
        // measurement contract says: findings are printed for a human, and
        // the only assertion is that the *contract* held — every baselined
        // metric was measured in its declared unit. Values are benchmark
        // evidence; a noisy CI runner must never turn them into a red build.
        let baseline: Baseline = ron::from_str(include_str!("../../perf/corpus-baseline.ron"))
            .expect("the perf baseline parses");
        let findings = compare(&capture, &baseline);
        eprintln!("{}", render(&findings));
        assert_ne!(
            worst(&findings),
            Verdict::Incomparable,
            "a baselined metric is missing or mis-united: {findings:?}"
        );
    }
}
