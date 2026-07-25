//! Time feel: bullet-time and hitstop.
//!
//! Aiming a broadside or a microwarp dilates global time toward a slow-motion
//! timescale while a rechargeable battery has charge, spent in real seconds so
//! the window stays a real ~5s regardless of the dilation. A landed kill instead
//! *stops* time dead for a few frames.
//!
//! Torpedo locking is the one aimed action that does **not** dilate — see
//! [`aim_time_dilation`].
//!
//! [`aim_time_dilation`] is the **only** writer of `Time<Virtual>`'s relative
//! speed. Both effects have to go through it, because two systems each setting
//! the timescale every frame would simply overwrite each other.

use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use vt_sim::prelude::PilotIntent;

use crate::input::{Aiming, Paused};
use crate::Player;

/// Fraction of normal time while aiming (bullet-time).
const AIM_TIMESCALE: f32 = 0.10;
/// Fraction of normal time during a hitstop — near enough a freeze-frame.
const HITSTOP_TIMESCALE: f32 = 0.05;

/// A brief freeze-frame on impact, measured in *real* seconds so it lasts the
/// same handful of frames whether or not bullet-time is already running.
#[derive(Resource, Default)]
pub struct Hitstop {
    remaining: f32,
}

impl Hitstop {
    /// Freeze for `secs`. The longest pending freeze wins, so a kill landing
    /// during an earlier hit's stop extends it rather than cutting it short.
    pub fn freeze(&mut self, secs: f32) {
        self.remaining = self.remaining.max(secs);
    }
}

/// The aim battery: aiming a weapon dilates global time toward
/// [`AIM_TIMESCALE`] while this has charge, then eases back. Charge is spent in
/// real seconds (so the window is a real ~5s regardless of the dilation) and
/// recovers when not aiming. `dilation` is the current eased timescale.
#[derive(Resource)]
pub struct AimBattery {
    pub charge: f32,
    pub max: f32,
    drain_per_sec: f32,
    recharge_per_sec: f32,
    dilation: f32,
}

impl Default for AimBattery {
    fn default() -> Self {
        Self {
            charge: 5.0,
            max: 5.0,
            drain_per_sec: 1.0,
            recharge_per_sec: 1.0,
            dilation: 1.0,
        }
    }
}

/// Dilate global time toward [`AIM_TIMESCALE`] while the player is aiming and the
/// aim battery has charge; ease back otherwise. Battery + easing run on real
/// time so the window is a real ~5s and stays smooth under dilation.
pub fn aim_time_dilation(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    paused: Res<Paused>,
    aiming: Res<Aiming>,
    player: Query<&PilotIntent, With<Player>>,
    mut battery: ResMut<AimBattery>,
    mut hitstop: ResMut<Hitstop>,
) {
    // While paused — or parked on the start screen — the virtual clock is
    // frozen. Leave the battery untouched and don't fight the freeze with a
    // speed change.
    if paused.0 || virt.is_paused() {
        return;
    }
    let dt = real.delta_secs();
    let microwarp_hold = player.single().map(|p| p.microwarp_hold).unwrap_or(false);
    // Torpedo locking is deliberately *not* here. Locks accrue on a timer while
    // the pilot sweeps the cursor, so slowing time would only stretch the sweep
    // out in real seconds while draining the battery for nothing — and it made
    // the one ability you want to line up carefully the one that punished you
    // for taking your time.
    let wants_dilation = aiming.port || aiming.starboard || microwarp_hold;
    let target = if wants_dilation && battery.charge > 0.0 {
        battery.charge = (battery.charge - battery.drain_per_sec * dt).max(0.0);
        AIM_TIMESCALE
    } else {
        battery.charge = (battery.charge + battery.recharge_per_sec * dt).min(battery.max);
        1.0
    };
    let k = 1.0 - (-10.0 * dt).exp();
    battery.dilation += (target - battery.dilation) * k;

    // A hitstop overrides the eased dilation outright — the point is the abrupt
    // stop, so easing into it would defeat it. The aim battery keeps easing
    // underneath, so time resumes wherever the dilation had got to.
    if hitstop.remaining > 0.0 {
        hitstop.remaining -= dt;
        virt.set_relative_speed(HITSTOP_TIMESCALE);
        return;
    }
    virt.set_relative_speed(battery.dilation.max(0.02));
}
