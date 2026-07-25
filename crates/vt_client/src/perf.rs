//! Live gameplay profiling, behind the `perf-capture` feature.
//!
//! The collector stays out of the sim, exactly as the fleet's measurement
//! contract requires: one presentation-side system samples the frame delta,
//! and on exit the capture — provenance, raw series, summaries — is written
//! to `perf-capture.json` (gitignored). A live capture diffs against the
//! same baselines a harness run does; the numbers themselves are benchmark
//! evidence, never assertions.

use bevy::prelude::*;
use vellum_perf::{Profile, Recorder, Unit};

/// Where the capture lands. In the repo root and gitignored, like the other
/// runtime debris this repo's hygiene rules already name.
const CAPTURE_PATH: &str = "perf-capture.json";

#[derive(Resource, Default)]
struct FrameRecorder(Recorder);

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameRecorder>()
            .add_systems(Update, (sample_frame, write_capture_on_exit).chain());
    }
}

/// One sample per rendered frame — presentation cost, not sim cost. The sim
/// runs in FixedUpdate; a frame that carried several fixed steps shows up
/// here as one expensive frame, which is what the player felt.
fn sample_frame(time: Res<Time>, mut recorder: ResMut<FrameRecorder>) {
    let ms = time.delta_secs_f64() * 1000.0;
    // The first frame's delta is startup noise, not gameplay.
    if ms > 0.0 {
        recorder.0.sample("frame-ms", Unit::Millis, ms);
    }
}

/// On exit, close the recorder into a capture and write it. Reading the
/// AppExit message here (rather than an exit hook) keeps the whole plugin an
/// ordinary pair of systems.
fn write_capture_on_exit(mut exits: MessageReader<AppExit>, mut recorder: ResMut<FrameRecorder>) {
    if exits.read().next().is_none() {
        return;
    }
    let taken = std::mem::take(&mut recorder.0);
    if taken.is_empty() {
        return;
    }
    let capture = taken.finish(
        "live",
        Profile {
            runtime: "native".into(),
            build: if cfg!(debug_assertions) {
                "dev".into()
            } else {
                "release".into()
            },
            ..Profile::default()
        },
    );
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(error) = std::fs::write(CAPTURE_PATH, capture.to_json()) {
        warn!("perf capture not written to {CAPTURE_PATH}: {error}");
    } else {
        info!("perf capture written to {CAPTURE_PATH}");
    }
    #[cfg(target_arch = "wasm32")]
    let _ = capture;
}
