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

use bevy::prelude::*;
use vt_sim::prelude::{Plunder, Projectile, ShipDestroyed, ShipHit};

/// The sound effects the game can play.
#[derive(Clone, Copy)]
pub enum Sound {
    Broadside,
    Hit,
    Explosion,
    Board,
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
        }
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

        app.add_systems(Update, (sfx_broadside, sfx_hits, sfx_explosions, sfx_board));
    }
}

// ---- Trigger systems (shared across platforms) ----

/// A broadside thump whenever new cannonballs appear (once per frame of firing).
fn sfx_broadside(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    new_shots: Query<(), Added<Projectile>>,
) {
    if !new_shots.is_empty() {
        play_sound(&mut commands, &sfx, Sound::Broadside);
    }
}

/// A hit tick when a hull is struck.
fn sfx_hits(mut commands: Commands, sfx: Res<SfxAssets>, mut hits: MessageReader<ShipHit>) {
    if hits.read().count() > 0 {
        play_sound(&mut commands, &sfx, Sound::Hit);
    }
}

/// A blast when a ship is destroyed.
fn sfx_explosions(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    mut destroyed: MessageReader<ShipDestroyed>,
) {
    if destroyed.read().count() > 0 {
        play_sound(&mut commands, &sfx, Sound::Explosion);
    }
}

/// A chime when a ship is boarded (the plunder tally ticks up).
fn sfx_board(
    mut commands: Commands,
    sfx: Res<SfxAssets>,
    plunder: Res<Plunder>,
    mut last: Local<u32>,
) {
    if plunder.ships_boarded > *last {
        play_sound(&mut commands, &sfx, Sound::Board);
    }
    *last = plunder.ships_boarded;
}

// ---- Native backend: synthesised WAV played through Bevy audio ----

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::Sound;
    use bevy::prelude::*;
    use std::f32::consts::TAU;

    /// Sample rate for the synthesised effects.
    const RATE: u32 = 22_050;

    /// Handles to the four pre-rendered sound effects.
    #[derive(Resource)]
    pub struct SfxAssets {
        broadside: Handle<AudioSource>,
        hit: Handle<AudioSource>,
        explosion: Handle<AudioSource>,
        board: Handle<AudioSource>,
    }

    impl SfxAssets {
        pub fn handle(&self, sound: Sound) -> Handle<AudioSource> {
            match sound {
                Sound::Broadside => self.broadside.clone(),
                Sound::Hit => self.hit.clone(),
                Sound::Explosion => self.explosion.clone(),
                Sound::Board => self.board.clone(),
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
        });
    }

    pub fn play_sound(commands: &mut Commands, sfx: &SfxAssets, sound: Sound) {
        commands.spawn((AudioPlayer(sfx.handle(sound)), PlaybackSettings::DESPAWN));
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
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
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
        fn vt_play_sound(name: &str);
    }

    /// No stored assets on the web — the shim owns synthesis.
    #[derive(Resource, Default)]
    pub struct SfxAssets;

    pub fn play_sound(_commands: &mut Commands, _sfx: &SfxAssets, sound: super::Sound) {
        vt_play_sound(sound.name());
    }
}

#[cfg(target_arch = "wasm32")]
use web::{play_sound, SfxAssets};
