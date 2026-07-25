//! Bullet-time: aiming a weapon dilates global time toward a slow-motion
//! timescale while a rechargeable battery has charge, spent in real seconds so
//! the window stays a real ~5s regardless of the dilation.

use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use vt_sim::prelude::PilotIntent;

use crate::input::{Aiming, Paused};
use crate::Player;

/// Fraction of normal time while aiming (bullet-time).
const AIM_TIMESCALE: f32 = 0.10;

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
) {
    // While paused the virtual clock is frozen; leave the battery untouched and
    // don't fight the pause with a speed change.
    if paused.0 {
        return;
    }
    let dt = real.delta_secs();
    let (torpedo_hold, microwarp_hold) = player
        .single()
        .map(|p| (p.torpedo_hold, p.microwarp_hold))
        .unwrap_or((false, false));
    let wants_dilation = aiming.port || aiming.starboard || torpedo_hold || microwarp_hold;
    let target = if wants_dilation && battery.charge > 0.0 {
        battery.charge = (battery.charge - battery.drain_per_sec * dt).max(0.0);
        AIM_TIMESCALE
    } else {
        battery.charge = (battery.charge + battery.recharge_per_sec * dt).min(battery.max);
        1.0
    };
    let k = 1.0 - (-10.0 * dt).exp();
    battery.dilation += (target - battery.dilation) * k;
    virt.set_relative_speed(battery.dilation.max(0.02));
}
