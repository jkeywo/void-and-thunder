//! Sound effects — cross-platform, no asset files.
//!
//! The sounds are synthesised procedurally. Two backends, chosen at compile
//! time:
//!
//! - **Native** — the sounds are rendered to in-memory WAV buffers and played
//!   through Bevy's audio (`AudioPlayer`).
//! - **Web (wasm)** — Bevy's audio is disabled (it misbehaves in the browser),
//!   so each sound is played by a small WebAudio shim in `index.html`, called
//!   via `wasm-bindgen` (`window.vtPlaySound(name)`).
//!
//! The trigger systems (which game moment plays which sound) are shared; only
//! `play_sound` and the asset storage differ per platform.
//!
//! **Every sound is played through [`Shot`], never bare.** A fixed sample fired
//! at a fixed pitch and volume is the single most obvious tell that a game's
//! audio is synthetic: fire two broadsides and the ear hears one recording
//! twice, not two events. So each play carries a pitch jitter and a gain, and
//! the gain falls off with distance from the camera — a kill across the system
//! should not be as loud as one alongside.

use bevy::prelude::*;
use bevy::time::Real;
use vt_sim::prelude::{
    lcg_next, BoostDrive, Brace, Hull, MicrowarpDrive, Plunder, Projectile, ShipDestroyed, ShipHit,
    SimTuning,
};

use crate::data::FeelTuning;
use crate::Player;

/// The sound effects the game can play.
#[derive(Clone, Copy)]
pub enum Sound {
    Broadside,
    Hit,
    Explosion,
    Board,
    /// The boost drive lighting up.
    Boost,
    /// A microwarp discharging.
    Warp,
    /// Bracing: the crew slamming the shutters down.
    Brace,
    /// Hull integrity critical — a slow, unwelcome pulse.
    HullWarning,
}

impl Sound {
    /// Stable name passed to the WebAudio shim on the web. Only used by the
    /// wasm backend.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn name(self) -> &'static str {
        match self {
            Sound::Broadside => "broadside",
            Sound::Hit => "hit",
            Sound::Explosion => "explosion",
            Sound::Board => "board",
            Sound::Boost => "boost",
            Sound::Warp => "warp",
            Sound::Brace => "brace",
            Sound::HullWarning => "hullwarn",
        }
    }
}

/// One playback of a sound: which one, how fast, how loud.
#[derive(Clone, Copy)]
pub struct Shot {
    pub sound: Sound,
    /// Playback rate. 1.0 is the sample as synthesised.
    pub pitch: f32,
    /// Linear gain. 1.0 is as synthesised.
    pub gain: f32,
}

impl Shot {
    /// A plain play at the authored pitch and volume.
    pub fn new(sound: Sound) -> Self {
        Self {
            sound,
            pitch: 1.0,
            gain: 1.0,
        }
    }

    /// Jitter the pitch by up to `spread` either way, from `seed`.
    pub fn varied(mut self, seed: &mut u32, spread: f32) -> Self {
        self.pitch *= 1.0 + (lcg_next(seed) - 0.5) * 2.0 * spread;
        self
    }

    /// Attenuate for an event `distance` away from the listener.
    ///
    /// The curve is deliberately the same `shake_range` the camera shake uses,
    /// so "how far away before it stops mattering" is one number for the whole
    /// game rather than one per sense. Never quite silent inside the range —
    /// a floor keeps a distant kill audible as a rumble.
    pub fn at_distance(mut self, distance: f32, range: f32) -> Self {
        let near = (1.0 - distance / range).clamp(0.0, 1.0);
        self.gain *= 0.12 + 0.88 * near * near;
        self
    }
}

/// Registers the sound-effect backend and the systems that trigger sounds.
pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, setup_sfx_assets);
        #[cfg(target_arch = "wasm32")]
        app.init_resource::<SfxAssets>();

        app.add_systems(
            Update,
            (
                sfx_broadside,
                sfx_hits,
                sfx_explosions,
                sfx_board,
                sfx_abilities,
                sfx_hull_warning,
            ),
        );
    }
}

/// Most sounds land within ±8% of their authored pitch — enough that two of the
/// same event never quite match, not so much that a cannon sounds like a
/// different cannon each time.
const PITCH_SPREAD: f32 = 0.08;

/// At most this many of the same sound in one frame. A volley of three guns
/// landing together should thicken the hit, but eight simultaneous copies of one
/// short sample just clip into a click.
const MAX_STACK: usize = 3;

/// Where the listener is — the player's ship, or the origin before one exists.
fn listener(player: &Query<&Transform, With<Player>>) -> Vec2 {
    player
        .single()
        .map(|t| t.translation.truncate())
        .unwrap_or(Vec2::ZERO)
}

// ---- Trigger systems (shared across platforms) ----

/// A broadside thump whenever new cannonballs appear (once per frame of firing).
fn sfx_broadside(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    mut seed: Local<u32>,
    feel: Res<FeelTuning>,
    player: Query<&Transform, With<Player>>,
    new_shots: Query<&Transform, Added<Projectile>>,
) {
    // One thump per volley, not per ball: the guns of a broadside fire together
    // and the ear reads them as a single report.
    let Some(shot) = new_shots.iter().next() else {
        return;
    };
    let distance = shot.translation.truncate().distance(listener(&player));
    play_sound(
        &mut commands,
        &sfx,
        Shot::new(Sound::Broadside)
            .varied(&mut seed, PITCH_SPREAD)
            .at_distance(distance, feel.impact.shake_range),
    );
}

/// A hit tick when a hull is struck, thickening slightly when several land at
/// once but never stacking into a clipped mess.
fn sfx_hits(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    mut seed: Local<u32>,
    feel: Res<FeelTuning>,
    player: Query<&Transform, With<Player>>,
    mut hits: MessageReader<ShipHit>,
) {
    let here = listener(&player);
    for hit in hits.read().take(MAX_STACK) {
        play_sound(
            &mut commands,
            &sfx,
            Shot::new(Sound::Hit)
                .varied(&mut seed, PITCH_SPREAD * 1.5)
                .at_distance(hit.position.distance(here), feel.impact.shake_range),
        );
    }
}

/// A blast when a ship is destroyed. Pitched down a little for a big kill so a
/// capital hull sounds heavier than a picket.
fn sfx_explosions(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    mut seed: Local<u32>,
    feel: Res<FeelTuning>,
    player: Query<&Transform, With<Player>>,
    mut destroyed: MessageReader<ShipDestroyed>,
) {
    let here = listener(&player);
    for kill in destroyed.read().take(MAX_STACK) {
        play_sound(
            &mut commands,
            &sfx,
            Shot::new(Sound::Explosion)
                .varied(&mut seed, PITCH_SPREAD)
                .at_distance(kill.position.distance(here), feel.impact.shake_range),
        );
    }
}

/// A chime when a ship is boarded (the plunder tally ticks up). Not varied and
/// not attenuated: this one is a UI confirmation, not a thing in the world.
fn sfx_board(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    plunder: Res<Plunder>,
    mut last: Local<u32>,
) {
    if plunder.ships_boarded > *last {
        play_sound(&mut commands, &sfx, Shot::new(Sound::Board));
    }
    *last = plunder.ships_boarded;
}

/// Boost, microwarp and brace — the player's own kit, so always full volume and
/// keyed off the *edge* of each state rather than the state itself.
fn sfx_abilities(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    mut seed: Local<u32>,
    mut was: Local<(bool, bool, f32)>,
    player: Query<(&BoostDrive, &Brace, &MicrowarpDrive), With<Player>>,
) {
    let Ok((boost, brace, warp)) = player.single() else {
        // No ship: forget the last state so respawning does not fire a stale edge.
        *was = (false, false, 0.0);
        return;
    };
    let (was_boosting, was_bracing, last_warp_timer) = *was;

    let boosting = boost.active;
    if boosting && !was_boosting {
        play_sound(
            &mut commands,
            &sfx,
            Shot::new(Sound::Boost).varied(&mut seed, PITCH_SPREAD),
        );
    }
    if brace.active && !was_bracing {
        play_sound(&mut commands, &sfx, Shot::new(Sound::Brace));
    }
    // The cooldown jumping from ~0 up to its full duration is the warp firing.
    if warp.timer > last_warp_timer + 0.01 {
        play_sound(&mut commands, &sfx, Shot::new(Sound::Warp));
    }

    *was = (boosting, brace.active, warp.timer);
}

/// A slow pulse while the player's hull is critical. Deliberately sparse — an
/// alarm that fires every frame stops being information and becomes noise.
fn sfx_hull_warning(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    time: Res<Time<Real>>,
    tuning: Res<SimTuning>,
    mut next_at: Local<f32>,
    player: Query<&Hull, With<Player>>,
) {
    let Ok(hull) = player.single() else {
        *next_at = 0.0;
        return;
    };
    let frac = (hull.current / hull.max).clamp(0.0, 1.0);
    if frac > tuning.cripple_threshold {
        *next_at = 0.0;
        return;
    }
    let now = time.elapsed_secs();
    if now < *next_at {
        return;
    }
    // Quickens as the hull fails: ~1.4s at the threshold down to ~0.6s at death.
    let interval = 0.6 + 0.8 * (frac / tuning.cripple_threshold).clamp(0.0, 1.0);
    *next_at = now + interval;
    play_sound(&mut commands, &sfx, Shot::new(Sound::HullWarning));
}

// ---- Native backend: synthesised WAV played through Bevy audio ----

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{Shot, Sound};
    use bevy::prelude::*;
    use std::f32::consts::TAU;

    /// Sample rate for the synthesised effects.
    const RATE: u32 = 22_050;

    /// Handles to the pre-rendered sound effects.
    #[derive(Resource)]
    pub struct SfxAssets {
        broadside: Handle<AudioSource>,
        hit: Handle<AudioSource>,
        explosion: Handle<AudioSource>,
        board: Handle<AudioSource>,
        boost: Handle<AudioSource>,
        warp: Handle<AudioSource>,
        brace: Handle<AudioSource>,
        hull_warning: Handle<AudioSource>,
    }

    impl SfxAssets {
        pub fn handle(&self, sound: Sound) -> Handle<AudioSource> {
            match sound {
                Sound::Broadside => self.broadside.clone(),
                Sound::Hit => self.hit.clone(),
                Sound::Explosion => self.explosion.clone(),
                Sound::Board => self.board.clone(),
                Sound::Boost => self.boost.clone(),
                Sound::Warp => self.warp.clone(),
                Sound::Brace => self.brace.clone(),
                Sound::HullWarning => self.hull_warning.clone(),
            }
        }
    }

    /// Startup: render each sound to a WAV buffer and store its asset handle.
    pub fn setup_sfx_assets(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
        let mut make = |samples: Vec<f32>| {
            sources.add(AudioSource {
                bytes: wav_from_samples(&samples).into(),
            })
        };
        commands.insert_resource(SfxAssets {
            broadside: make(synth_broadside()),
            hit: make(synth_hit()),
            explosion: make(synth_explosion()),
            board: make(synth_board()),
            boost: make(synth_boost()),
            warp: make(synth_warp()),
            brace: make(synth_brace()),
            hull_warning: make(synth_hull_warning()),
        });
    }

    /// Bevy resamples on `speed`, so a pitch shift also shortens or lengthens the
    /// sound — which is what we want: a higher-pitched impact is a sharper one.
    pub fn play_sound(commands: &mut Commands, sfx: &SfxAssets, shot: Shot) {
        if shot.gain <= 0.001 {
            return;
        }
        commands.spawn((
            AudioPlayer(sfx.handle(shot.sound)),
            PlaybackSettings::DESPAWN
                .with_speed(shot.pitch.max(0.05))
                .with_volume(bevy::audio::Volume::Linear(shot.gain)),
        ));
    }

    /// Encode mono f32 samples as a 16-bit PCM WAV byte buffer.
    fn wav_from_samples(samples: &[f32]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut v = Vec::with_capacity(44 + data_len as usize);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&RATE.to_le_bytes());
        v.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
        v.extend_from_slice(&2u16.to_le_bytes()); // block align
        v.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            let i = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            v.extend_from_slice(&i.to_le_bytes());
        }
        v
    }

    /// A cheap white-noise generator for the synths.
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            vt_sim::prelude::lcg_next(&mut self.0) * 2.0 - 1.0
        }
    }

    fn secs(t: f32) -> usize {
        (RATE as f32 * t) as usize
    }

    /// Low sweep + filtered noise: a cannon thump.
    fn synth_broadside() -> Vec<f32> {
        let mut noise = Noise(0x2468);
        (0..secs(0.28))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (-t * 12.0).exp();
                let freq = 120.0 - 70.0 * (t / 0.28);
                let low = (t * freq * TAU).sin();
                (low * 0.6 + noise.next() * 0.4) * env * 0.6
            })
            .collect()
    }

    /// A short high square-wave tick.
    fn synth_hit() -> Vec<f32> {
        (0..secs(0.09))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (-t * 40.0).exp();
                (t * 760.0 * TAU).sin().signum() * env * 0.4
            })
            .collect()
    }

    /// A low-passed noise burst: an explosion.
    fn synth_explosion() -> Vec<f32> {
        let mut noise = Noise(0x1357);
        let mut lp = 0.0f32;
        (0..secs(0.5))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (-t * 6.0).exp();
                lp += (noise.next() - lp) * 0.2;
                lp * env * 0.7
            })
            .collect()
    }

    /// A two-note rising chime: boarding.
    fn synth_board() -> Vec<f32> {
        (0..secs(0.34))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let (freq, env) = if t < 0.12 {
                    (523.0, (-t * 8.0).exp())
                } else {
                    (784.0, (-(t - 0.12) * 8.0).exp())
                };
                (t * freq * TAU).sin() * env * 0.4
            })
            .collect()
    }

    /// A rising whoosh: the boost drive catching. Noise swept upward through a
    /// one-pole filter, with a rising tone under it for pitch.
    fn synth_boost() -> Vec<f32> {
        let mut noise = Noise(0x9ABC);
        let mut lp = 0.0f32;
        let dur = 0.45;
        (0..secs(dur))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let p = t / dur;
                // Swell in, then tail off — the drive biting rather than a bang.
                let env = (p * 4.0).min(1.0) * (1.0 - p).powf(1.5);
                let k = 0.04 + 0.30 * p; // the filter opening as it spools up
                lp += (noise.next() - lp) * k;
                let tone = (t * (90.0 + 260.0 * p) * TAU).sin();
                (lp * 0.7 + tone * 0.3) * env * 0.5
            })
            .collect()
    }

    /// A downward-swept zap with a snap on the front: the microwarp discharging.
    fn synth_warp() -> Vec<f32> {
        let mut noise = Noise(0x5E1F);
        let dur = 0.38;
        (0..secs(dur))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let p = t / dur;
                let env = (-t * 7.0).exp();
                // Falling pitch reads as departure; the flat sweep is the tear.
                let freq = 900.0 * (1.0 - p * 0.85) + 60.0;
                let tone = (t * freq * TAU).sin();
                let snap = if t < 0.03 { noise.next() * 0.5 } else { 0.0 };
                (tone * 0.55 + snap) * env * 0.45
            })
            .collect()
    }

    /// A short metallic clang: shutters coming down.
    fn synth_brace() -> Vec<f32> {
        // Two detuned partials beating against each other give the ring a metal
        // edge that a single sine cannot.
        (0..secs(0.22))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (-t * 18.0).exp();
                let a = (t * 320.0 * TAU).sin();
                let b = (t * 487.0 * TAU).sin();
                (a * 0.6 + b * 0.4) * env * 0.4
            })
            .collect()
    }

    /// A low two-pulse warble: the hull is failing. Quiet and dull on purpose —
    /// it repeats, so anything bright would grate within three cycles.
    fn synth_hull_warning() -> Vec<f32> {
        (0..secs(0.30))
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = if t < 0.13 {
                    (-t * 11.0).exp()
                } else {
                    (-(t - 0.15) * 11.0).exp() * if t < 0.15 { 0.0 } else { 1.0 }
                };
                (t * 196.0 * TAU).sin() * env * 0.3
            })
            .collect()
    }
}

#[cfg(not(target_arch = "wasm32"))]
use native::{play_sound, setup_sfx_assets, SfxAssets};

// ---- Web backend: WebAudio shim in index.html, called via wasm-bindgen ----

#[cfg(target_arch = "wasm32")]
mod web {
    use bevy::prelude::*;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = window, js_name = vtPlaySound)]
        fn vt_play_sound(name: &str, pitch: f32, gain: f32);
    }

    /// No stored assets on the web — the shim owns synthesis.
    #[derive(Resource, Default)]
    pub struct SfxAssets;

    pub fn play_sound(_commands: &mut Commands, _sfx: &SfxAssets, shot: super::Shot) {
        if shot.gain <= 0.001 {
            return;
        }
        vt_play_sound(shot.sound.name(), shot.pitch.max(0.05), shot.gain);
    }
}

#[cfg(target_arch = "wasm32")]
use web::{play_sound, SfxAssets};
