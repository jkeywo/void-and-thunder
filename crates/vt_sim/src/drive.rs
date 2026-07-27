//! Ship drives, the battery that runs them, and per-ship speed scaling.
//!
//! Four systems run each step before movement:
//! - [`emp_recovery_system`] bleeds EMP damage back off every hull.
//! - [`battery_draw_system`] is the *only* place a battery is spent: it walks
//!   the fitted devices in a fixed order and powers each one it can pay for.
//! - [`battery_recharge_system`] tops the pool up, but only in a step where
//!   nothing drew from it.
//! - [`speed_scale_system`] folds EMP and boost into each ship's [`SpeedScale`],
//!   which [`movement_system`](crate::ship::movement_system) then applies.
//!
//! The maths that decides the multiplier is the pure [`speed_scale`] so it can be
//! unit-tested without a `World`.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::components::{Anchored, EmpDefense, PilotIntent, Ship, SpeedScale, ENGAGEMENT_RANGE};
use crate::emp::EmpWeapon;
use crate::point_defense::PointDefense;

/// One pool of charge, shared by every device in the ship's battery slot.
///
/// A ship fits exactly one battery device — the boost drive, the disruptor or
/// the point-defence emitter — and they all draw from here, so the gauge means
/// the same thing whatever is bolted on and swapping the fit never changes what
/// the pilot is reading. A device on a hull with no battery is simply never
/// powered: there is nothing to run it on.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Battery {
    pub charge: f32,
    pub max: f32,
    pub recharge_per_sec: f32,
    /// Set by [`battery_draw_system`] when any device drew this step and cleared
    /// by [`battery_recharge_system`]. Live state — never authored, and the
    /// reason a spent battery does not also recharge in the same step.
    #[serde(skip)]
    pub drawn: bool,
}

impl Default for Battery {
    fn default() -> Self {
        Self {
            charge: 3.0,
            max: 3.0,
            recharge_per_sec: 0.6,
            drawn: false,
        }
    }
}

impl Battery {
    /// Spend up to `amount`. Returns whether there was anything to spend — the
    /// caller uses that as "am I powered this step", so a device asking for more
    /// than is left still gets its last partial step of work out of the pool.
    pub fn draw(&mut self, amount: f32) -> bool {
        if self.charge <= 0.0 {
            return false;
        }
        self.charge = (self.charge - amount).max(0.0);
        self.drawn = true;
        true
    }

    /// Remaining charge as a fraction of capacity.
    pub fn fraction(&self) -> f32 {
        (self.charge / self.max.max(0.01)).clamp(0.0, 1.0)
    }
}

/// A rechargeable overdrive. While the pilot holds it and the ship's [`Battery`]
/// can pay, speed is multiplied by `multiplier`. Config + live state live
/// together so the controller just flips `active`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct BoostDrive {
    pub multiplier: f32,
    pub drain_per_sec: f32,
    /// Set by the controller each frame — is the pilot holding boost?
    pub active: bool,
    /// Set by [`battery_draw_system`]: held *and* paid for. Live state.
    #[serde(skip)]
    pub engaged: bool,
}

impl Default for BoostDrive {
    fn default() -> Self {
        Self {
            multiplier: 1.6,
            drain_per_sec: 1.0,
            active: false,
            engaged: false,
        }
    }
}

impl BoostDrive {
    /// True when boost is actually contributing thrust this frame.
    ///
    /// A method over the field of the same name so the presentation layer can go
    /// on passing `BoostDrive::engaged` as a function.
    pub fn engaged(&self) -> bool {
        self.engaged
    }
}

/// A short-range teleport drive. Holding aim previews a destination within
/// `range`; releasing warps there (gated by `cooldown`). Player-only.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct MicrowarpDrive {
    pub range: f32,
    pub cooldown: f32,
    pub timer: f32,
    pub was_holding: bool,
}

impl Default for MicrowarpDrive {
    fn default() -> Self {
        Self {
            range: ENGAGEMENT_RANGE,
            cooldown: 2.0,
            timer: 0.0,
            was_holding: false,
        }
    }
}

/// The speed multiplier for a ship given its EMP load and (optional) boost.
pub fn speed_scale(emp: &EmpDefense, boost: Option<&BoostDrive>) -> f32 {
    let boost_factor = match boost {
        Some(b) if b.engaged => b.multiplier,
        _ => 1.0,
    };
    emp.speed_factor() * boost_factor
}

/// Bevy system: bleed EMP damage back off every hull.
///
/// Split out of the old `battery_system` because it has nothing to do with the
/// battery — it is how a ship recovers from being shot at, not how it spends.
pub fn emp_recovery_system(time: Res<Time>, mut ships: Query<&mut EmpDefense>) {
    let dt = time.delta_secs();
    for mut emp in &mut ships {
        emp.damage = (emp.damage - emp.recovery_per_sec * dt).max(0.0);
    }
}

/// Bevy system: spend the battery on whichever devices are fitted and held.
///
/// The one place charge leaves the pool. Devices are walked in a fixed order —
/// boost, then disruptor, then point defence — so that a hull carrying more than
/// one of them (which the catalogue never fits, but the data model allows)
/// drains predictably rather than in whatever order the archetype iterates.
///
/// Runs in `SimSet::Systems`, ahead of both `speed_scale_system` and the weapons
/// pass, so a device powered this step also *acts* this step.
pub fn battery_draw_system(
    time: Res<Time>,
    mut ships: Query<(
        &PilotIntent,
        &mut Battery,
        Option<&mut BoostDrive>,
        Option<&mut EmpWeapon>,
        Option<&mut PointDefense>,
    )>,
) {
    let dt = time.delta_secs();
    for (intent, mut battery, boost, emp, screen) in &mut ships {
        if let Some(mut boost) = boost {
            boost.engaged = boost.active && battery.draw(boost.drain_per_sec * dt);
        }
        if let Some(mut emp) = emp {
            emp.powered = intent.emp_fire && battery.draw(emp.drain_per_sec * dt);
        }
        if let Some(mut screen) = screen {
            screen.powered = intent.point_defense_fire && battery.draw(screen.drain_per_sec * dt);
        }
    }
}

/// Bevy system: recharge every battery that was left alone this step.
///
/// Deliberately a separate pass after [`battery_draw_system`]: a pool that paid
/// for something must not also be topped up in the same step, or holding a
/// device down would cost only the difference between the two rates.
pub fn battery_recharge_system(time: Res<Time>, mut batteries: Query<&mut Battery>) {
    let dt = time.delta_secs();
    for mut battery in &mut batteries {
        if !battery.drawn {
            battery.charge = (battery.charge + battery.recharge_per_sec * dt).min(battery.max);
        }
        battery.drawn = false;
    }
}

/// Bevy system: recompute every ship's [`SpeedScale`].
pub fn speed_scale_system(
    mut ships: Query<(&mut SpeedScale, &EmpDefense, Option<&BoostDrive>), With<Ship>>,
) {
    for (mut scale, emp, boost) in &mut ships {
        scale.0 = speed_scale(emp, boost);
    }
}

/// Clamp `point` to lie within `range` of `origin`.
pub fn clamp_to_range(origin: Vec2, point: Vec2, range: f32) -> Vec2 {
    let delta = point - origin;
    if delta.length() > range {
        origin + delta.normalize_or_zero() * range
    } else {
        point
    }
}

/// Bevy system: teleport a microwarp ship to the aim point (clamped to range)
/// when the pilot releases, if the drive has cooled down. Reads each ship's own
/// [`PilotIntent`], so player and AI ships share the system.
pub fn microwarp_system(
    time: Res<Time>,
    mut ships: Query<(&mut Transform, &mut MicrowarpDrive, &PilotIntent), Without<Anchored>>,
) {
    let dt = time.delta_secs();
    for (mut transform, mut drive, intent) in &mut ships {
        drive.timer = (drive.timer - dt).max(0.0);
        let hold = intent.microwarp_hold;
        if drive.was_holding && !hold && drive.timer <= 0.0 {
            let origin = transform.translation.truncate();
            let dest = clamp_to_range(origin, intent.aim_point, drive.range);
            transform.translation.x = dest.x;
            transform.translation.y = dest.y;
            drive.timer = drive.cooldown;
        }
        drive.was_holding = hold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_ship_runs_at_full_speed() {
        assert!((speed_scale(&EmpDefense::default(), None) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn emp_inverse_lerps_speed() {
        let half = EmpDefense {
            resist: 100.0,
            damage: 50.0,
            recovery_per_sec: 0.0,
        };
        assert!((speed_scale(&half, None) - 0.5).abs() < 1e-6);
        let full = EmpDefense {
            damage: 100.0,
            ..half
        };
        assert!(speed_scale(&full, None).abs() < 1e-6);
    }

    #[test]
    fn boost_multiplies_when_engaged() {
        let emp = EmpDefense::default();
        let boost = BoostDrive {
            multiplier: 1.6,
            engaged: true,
            active: true,
            ..Default::default()
        };
        assert!((speed_scale(&emp, Some(&boost)) - 1.6).abs() < 1e-6);
        // Held but unpaid-for — a flat battery leaves `engaged` false upstream.
        let unpaid = BoostDrive {
            engaged: false,
            ..boost
        };
        assert!((speed_scale(&emp, Some(&unpaid)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn emp_and_boost_stack() {
        let emp = EmpDefense {
            resist: 100.0,
            damage: 50.0,
            recovery_per_sec: 0.0,
        };
        let boost = BoostDrive {
            multiplier: 1.6,
            engaged: true,
            active: true,
            ..Default::default()
        };
        assert!((speed_scale(&emp, Some(&boost)) - 0.8).abs() < 1e-6);
    }

    /// A ship fits one battery device, but the pool is the ship's — so two
    /// devices held at once must both come out of the same charge rather than
    /// each getting their own.
    #[test]
    fn one_pool_serves_two_devices() {
        let mut battery = Battery {
            charge: 10.0,
            ..Battery::default()
        };
        assert!(battery.draw(1.0));
        assert!(battery.draw(2.5));
        assert!((battery.charge - 6.5).abs() < 1e-6);
        assert!(battery.drawn, "a draw must mark the pool as spent-from");
    }

    /// The reason draw and recharge are separate passes: if a pool could do both
    /// in one step, holding a device down would only ever cost the difference
    /// between the two rates, and a 1.0/s drain against a 0.6/s recharge would
    /// last two and a half times as long as authored.
    #[test]
    fn a_device_that_drew_blocks_the_recharge_this_step() {
        use bevy_app::prelude::*;

        let mut app = App::new();
        app.init_resource::<Time>().add_systems(
            Update,
            (battery_draw_system, battery_recharge_system).chain(),
        );

        let ship = app
            .world_mut()
            .spawn((
                PilotIntent::default(),
                Battery {
                    charge: 5.0,
                    max: 5.0,
                    recharge_per_sec: 100.0,
                    drawn: false,
                },
                BoostDrive {
                    drain_per_sec: 1.0,
                    active: true,
                    ..BoostDrive::default()
                },
            ))
            .id();

        // A bare `Time` reports a zero delta until it is advanced, and a zero
        // delta spends nothing — step it by a tenth of a second by hand.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        app.update();

        let battery = app.world().get::<Battery>(ship).unwrap();
        assert!(
            (battery.charge - 4.9).abs() < 1e-5,
            "a tenth of a second at 1.0/s costs 0.1 and is not refunded by the \
             100/s recharge; got {}",
            battery.charge
        );
        assert!(!battery.drawn, "the flag must be cleared for the next step");
        assert!(
            app.world().get::<BoostDrive>(ship).unwrap().engaged,
            "a paid-for hold is engaged"
        );
    }

    /// Nothing left in the pool means the device does not run — the pilot can
    /// hold the key all they like.
    #[test]
    fn a_flat_battery_disengages_boost() {
        use bevy_app::prelude::*;

        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, battery_draw_system);

        let ship = app
            .world_mut()
            .spawn((
                PilotIntent::default(),
                Battery {
                    charge: 0.0,
                    ..Battery::default()
                },
                BoostDrive {
                    active: true,
                    ..BoostDrive::default()
                },
            ))
            .id();

        app.update();

        assert!(!app.world().get::<BoostDrive>(ship).unwrap().engaged);
    }

    /// A device with no battery behind it is never powered: there is nothing to
    /// run it on, and inventing free charge would make the slot meaningless.
    #[test]
    fn a_device_without_a_battery_never_powers_up() {
        use bevy_app::prelude::*;

        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, battery_draw_system);

        let ship = app
            .world_mut()
            .spawn((
                PilotIntent::default(),
                BoostDrive {
                    active: true,
                    ..BoostDrive::default()
                },
            ))
            .id();

        app.update();

        assert!(!app.world().get::<BoostDrive>(ship).unwrap().engaged);
    }

    #[test]
    fn microwarp_clamps_to_range() {
        // A point 1000 out with a 900 range lands on the edge.
        let dest = clamp_to_range(Vec2::ZERO, Vec2::new(1000.0, 0.0), 900.0);
        assert!((dest.x - 900.0).abs() < 1e-3);
        // A point inside range is unchanged.
        let near = clamp_to_range(Vec2::ZERO, Vec2::new(100.0, 0.0), 900.0);
        assert!((near.x - 100.0).abs() < 1e-6);
    }
}
