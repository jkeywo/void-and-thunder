//! Directional shields: a fore and an aft arc that soak damage before the hull.
//!
//! A shield is normally two independent banks, each covering 180° — everything
//! forward of the beam, and everything abaft it. They are separate pools on
//! purpose: a ship that has been running from a pursuer has a flat aft bank and
//! a full fore one, so the answer is to *turn and present the good side*, which
//! is the same decision the sail ladder and the broadside arcs already ask of
//! the player.
//!
//! A fit may instead be [`pooled`](Shield::pooled), which is the deliberate
//! opposite: one charge for the whole hull, so facing means nothing and the only
//! question left is whether you are hit at all. That is a different fight rather
//! than a softer one, and it is what the finale is built on.
//!
//! Damage goes through [`apply_hull_damage`](crate::combat::apply_hull_damage)
//! like everything else, so shields, bracing and invulnerability compose in one
//! place rather than each weapon having to remember all three.
//!
//! **A `max` of zero means no shields fitted.** Every ship carries the component
//! so the query shapes stay uniform, and a class turns shields on by authoring a
//! single number in `ships.ron`.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use bevy_time::Time;
use bevy_transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::components::{Heading, Ship};

/// Shield strength per arc for a hull that has them. The live value is
/// authored per class in `ships.ron`.
pub const SHIELD_MAX: f32 = 0.0;
/// Shield points restored per second, once regeneration resumes.
pub const SHIELD_REGEN_PER_SEC: f32 = 7.0;
/// Seconds a bank stays suppressed after taking a hit.
///
/// The delay is what makes shields a resource rather than a damage discount:
/// sustained fire on one side keeps that bank pinned at zero, so the pressure to
/// turn is constant. Without it, regeneration just lowers everyone's damage.
pub const SHIELD_REGEN_DELAY: f32 = 2.5;

/// Which half of the ship a blow landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShieldArc {
    /// Forward of the beam.
    Fore,
    /// Abaft the beam.
    Aft,
}

/// One arc's live state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
pub struct ShieldBank {
    /// Points remaining in this arc.
    pub charge: f32,
    /// Seconds left before regeneration resumes.
    pub cooldown: f32,
}

/// A ship's fore and aft shield arcs.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Shield {
    // Live state, never authored: a data file describes the *fit*, not how
    // charged it happens to be mid-fight. `ship_bundle` spawns every ship with
    // full banks via `charged`.
    #[serde(skip)]
    pub fore: ShieldBank,
    #[serde(skip)]
    pub aft: ShieldBank,
    /// Capacity of each arc, authored separately.
    ///
    /// Two numbers rather than one because a hull may be shielded on one side
    /// only — a siege platform with its whole generator pointed forward is a
    /// different problem to solve than an evenly-protected ship, and "get behind
    /// it" is a tactic worth being able to author. Both zero means no shields.
    ///
    /// On a `pooled` fit only `fore_max` is read: it is the whole pool.
    pub fore_max: f32,
    pub aft_max: f32,
    /// One shield covering everything, instead of two halves.
    ///
    /// A directional fit always leaves *somewhere* to stand: flatten the bank
    /// that faces you, swing round, and the other one is waiting at full charge
    /// while the first quietly recovers behind you. Even fitting both arcs
    /// equally does not remove that — it only hides it, since circling still
    /// buys a fresh bank each time.
    ///
    /// A pooled shield has no other side. Every blow draws on one charge and
    /// suppresses one timer, wherever it lands, so where you stand stops being a
    /// question and whether you get hit is the only one left. `fore_max` is the
    /// pool and `aft_max` is unused (see [`shield_refit_system`]).
    pub pooled: bool,
    pub regen_per_sec: f32,
    pub regen_delay: f32,
}

impl Default for Shield {
    fn default() -> Self {
        Self {
            fore: ShieldBank::default(),
            aft: ShieldBank::default(),
            fore_max: SHIELD_MAX,
            aft_max: SHIELD_MAX,
            pooled: false,
            regen_per_sec: SHIELD_REGEN_PER_SEC,
            regen_delay: SHIELD_REGEN_DELAY,
        }
    }
}

impl Shield {
    /// This fit with both banks full — how a ship enters the field.
    pub fn charged(self) -> Self {
        Self {
            fore: ShieldBank {
                charge: self.fore_max,
                cooldown: 0.0,
            },
            aft: ShieldBank {
                charge: self.aft_max,
                cooldown: 0.0,
            },
            ..self
        }
    }

    /// The bank that answers for a blow on `arc`.
    ///
    /// Normally the matching one; always the fore bank on a pooled fit, where
    /// there is only one and it covers everything. Every *read* of the shield
    /// goes through this, so a pooled hull reports the same charge whichever
    /// side is asked about — which is exactly what a single generator means, and
    /// keeps callers (the status ring, the pilot's choice of facing) from
    /// inventing a weak side that is not there.
    fn answering(&self, arc: ShieldArc) -> ShieldArc {
        if self.pooled {
            ShieldArc::Fore
        } else {
            arc
        }
    }

    /// Capacity of one arc, as authored. Not routed through
    /// [`Self::answering`] — this is the fit, not the protection.
    pub fn max_of(&self, arc: ShieldArc) -> f32 {
        match arc {
            ShieldArc::Fore => self.fore_max,
            ShieldArc::Aft => self.aft_max,
        }
    }

    /// Whether this hull has shields at all — on either side.
    pub fn fitted(&self) -> bool {
        self.fore_max > 0.0 || self.aft_max > 0.0
    }

    /// The bank protecting `arc` — the same one for both arcs when pooled.
    pub fn bank(&self, arc: ShieldArc) -> &ShieldBank {
        match self.answering(arc) {
            ShieldArc::Fore => &self.fore,
            ShieldArc::Aft => &self.aft,
        }
    }

    /// Whether the bank protecting `arc` is currently suppressed — it took a hit
    /// inside the regen delay and will not recover while it keeps taking them.
    pub fn suppressed(&self, arc: ShieldArc) -> bool {
        self.bank(arc).cooldown > 0.0
    }

    fn bank_mut(&mut self, arc: ShieldArc) -> &mut ShieldBank {
        match arc {
            ShieldArc::Fore => &mut self.fore,
            ShieldArc::Aft => &mut self.aft,
        }
    }

    /// This arc's charge as a fraction of *its own* capacity, for the HUD and
    /// the status ring. An unshielded arc reads zero rather than dividing by it.
    pub fn fraction(&self, arc: ShieldArc) -> f32 {
        let max = self.max_of(self.answering(arc));
        if max <= 0.0 {
            return 0.0;
        }
        (self.bank(arc).charge / max).clamp(0.0, 1.0)
    }

    /// Spend this arc against `damage`, returning what the shield could not stop
    /// and so passes through to the hull.
    ///
    /// Any contact suppresses the arc's regeneration, including one that only
    /// grazes a bank already at zero: being shot at is what keeps a shield down.
    pub fn absorb(&mut self, arc: ShieldArc, damage: f32) -> f32 {
        // Where the blow landed decides which bank answers — unless there is
        // only one, in which case it answers for everything.
        let arc = self.answering(arc);
        // A bare arc stops nothing and is not suppressed by being shot at —
        // there is no generator there to knock offline.
        if self.max_of(arc) <= 0.0 || damage <= 0.0 {
            return damage.max(0.0);
        }
        let delay = self.regen_delay;
        let bank = self.bank_mut(arc);
        bank.cooldown = delay;
        let taken = damage.min(bank.charge);
        bank.charge -= taken;
        damage - taken
    }
}

/// Which arc a blow landed on, given where the hull is pointing.
///
/// Decided by *where the blow struck* relative to the bow rather than by the
/// direction it was travelling: a shot that clips the stern while flying
/// forwards still hit the stern.
pub fn shield_arc(heading: f32, ship_pos: Vec2, impact: Vec2) -> ShieldArc {
    let forward = Vec2::from_angle(heading);
    // Dead-centre impacts (a ram contact can be) resolve to Fore rather than
    // needing a third case — the bow is where a ram lands anyway.
    if (impact - ship_pos).dot(forward) >= 0.0 {
        ShieldArc::Fore
    } else {
        ShieldArc::Aft
    }
}

/// Bevy system: tick each arc's suppression down, then regenerate it.
pub fn shield_regen_system(time: Res<Time>, mut ships: Query<&mut Shield, With<Ship>>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for mut shield in &mut ships {
        if !shield.fitted() {
            continue;
        }
        let rate = shield.regen_per_sec;
        for arc in [ShieldArc::Fore, ShieldArc::Aft] {
            let max = shield.max_of(arc);
            let bank = shield.bank_mut(arc);
            if bank.cooldown > 0.0 {
                bank.cooldown = (bank.cooldown - dt).max(0.0);
                continue;
            }
            bank.charge = (bank.charge + rate * dt).min(max);
        }
    }
}

/// Bevy system: keep a ship's shield fit consistent after a live edit.
///
/// The design panel can drag `max` while the game runs; without this a bank
/// stays stuck at whatever it held before the change, so lowering capacity
/// leaves an over-full arc and raising it looks like nothing happened.
pub fn shield_refit_system(mut ships: Query<&mut Shield, Changed<Shield>>) {
    for mut shield in &mut ships {
        // A pooled fit has no stern bank to hold anything. Zeroing the capacity
        // rather than trusting the author keeps `aft_max` from quietly becoming
        // a second pool nothing can ever reach — and makes a fit toggled to
        // pooled in the design panel behave the moment it is toggled.
        if shield.pooled && shield.aft_max != 0.0 {
            shield.aft_max = 0.0;
        }
        let (fore, aft) = (shield.fore_max, shield.aft_max);
        if shield.fore.charge > fore || shield.aft.charge > aft {
            shield.fore.charge = shield.fore.charge.min(fore);
            shield.aft.charge = shield.aft.charge.min(aft);
        }
    }
}

/// Reported alongside a hit so the presentation layer can tell a shield flare
/// from a hull breach — they want different colours and different weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageReport {
    /// Damage the shields soaked.
    pub to_shield: f32,
    /// Damage that reached the hull.
    pub to_hull: f32,
}

/// Bevy system helper: the arc a blow at `impact` lands on for a ship whose
/// transform and heading are known. Split out so the four damage call sites
/// share one definition of "which side is this".
pub fn arc_of_hit(transform: &Transform, heading: &Heading, impact: Vec2) -> ShieldArc {
    shield_arc(heading.0, transform.translation.truncate(), impact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    fn fitted() -> Shield {
        Shield {
            fore_max: 40.0,
            aft_max: 40.0,
            regen_per_sec: 10.0,
            regen_delay: 2.0,
            ..Default::default()
        }
        .charged()
    }

    #[test]
    fn an_unfitted_shield_stops_nothing() {
        let mut none = Shield::default();
        assert!(!none.fitted(), "the default fit is no shields at all");
        assert_eq!(
            none.absorb(ShieldArc::Fore, 30.0),
            30.0,
            "every point should pass straight through to the hull"
        );
    }

    #[test]
    fn a_shield_soaks_damage_then_lets_the_rest_through() {
        let mut s = fitted();
        assert_eq!(s.absorb(ShieldArc::Fore, 15.0), 0.0, "well within the bank");
        assert_eq!(s.fore.charge, 25.0);
        // 25 left against a 40-point blow: 15 reaches the hull.
        assert_eq!(s.absorb(ShieldArc::Fore, 40.0), 15.0);
        assert_eq!(s.fore.charge, 0.0);
    }

    #[test]
    fn the_arcs_are_independent() {
        let mut s = fitted();
        s.absorb(ShieldArc::Fore, 40.0);
        assert_eq!(s.fore.charge, 0.0, "the bow bank is spent");
        assert_eq!(
            s.aft.charge, 40.0,
            "the stern bank is untouched — turning about is the whole point"
        );
    }

    #[test]
    fn a_hit_suppresses_only_the_arc_that_took_it() {
        let mut s = fitted();
        s.absorb(ShieldArc::Aft, 5.0);
        assert_eq!(s.aft.cooldown, 2.0);
        assert_eq!(s.fore.cooldown, 0.0, "the far side keeps regenerating");
    }

    #[test]
    fn sustained_fire_keeps_a_flat_bank_down() {
        // A blow that a spent bank cannot reduce at all must still re-suppress
        // it, or a shield under continuous fire would tick back up between hits.
        let mut s = fitted();
        s.absorb(ShieldArc::Fore, 40.0);
        s.fore.cooldown = 0.0; // pretend the delay elapsed
        s.absorb(ShieldArc::Fore, 10.0);
        assert_eq!(s.fore.cooldown, 2.0, "being shot at keeps a shield down");
    }

    #[test]
    fn the_bow_half_is_fore_and_the_stern_half_is_aft() {
        // Facing +X from the origin.
        let at = |x: f32, y: f32| shield_arc(0.0, Vec2::ZERO, Vec2::new(x, y));
        assert_eq!(at(10.0, 0.0), ShieldArc::Fore, "dead ahead");
        assert_eq!(at(-10.0, 0.0), ShieldArc::Aft, "dead astern");
        assert_eq!(at(5.0, 9.0), ShieldArc::Fore, "forward of the port beam");
        assert_eq!(at(-5.0, 9.0), ShieldArc::Aft, "abaft the port beam");
        assert_eq!(at(0.0, 0.0), ShieldArc::Fore, "a dead-centre contact");
    }

    #[test]
    fn the_arcs_turn_with_the_hull() {
        // Facing +Y: a blow from +X is now on the starboard *beam*, which is the
        // boundary, and one from -Y is astern.
        let north = FRAC_PI_2;
        assert_eq!(
            shield_arc(north, Vec2::ZERO, Vec2::new(0.0, -10.0)),
            ShieldArc::Aft
        );
        assert_eq!(
            shield_arc(north, Vec2::ZERO, Vec2::new(0.0, 10.0)),
            ShieldArc::Fore
        );
        // Facing about (-X): a blow from +X is astern.
        assert_eq!(
            shield_arc(PI, Vec2::ZERO, Vec2::new(10.0, 0.0)),
            ShieldArc::Aft
        );
    }

    /// A pooled fit is one shield, so where a blow lands changes nothing about
    /// what stops it. This is the finale's whole defensive idea: no side of the
    /// hull is worth manoeuvring for.
    #[test]
    fn a_pooled_shield_has_no_other_side() {
        let mut s = Shield {
            pooled: true,
            fore_max: 100.0,
            aft_max: 0.0,
            regen_per_sec: 10.0,
            regen_delay: 2.0,
            ..Default::default()
        }
        .charged();

        // A blow on the stern spends the same charge a blow on the bow would.
        assert_eq!(s.absorb(ShieldArc::Aft, 40.0), 0.0);
        assert_eq!(s.fore.charge, 60.0, "the one pool paid for it");
        assert_eq!(
            s.fraction(ShieldArc::Fore),
            s.fraction(ShieldArc::Aft),
            "and both arcs report the same shield, because there is only one"
        );

        // Turning about finds no fresh bank waiting: what is left is what is
        // left, from any direction.
        assert_eq!(
            s.absorb(ShieldArc::Fore, 80.0),
            20.0,
            "60 stopped, 20 through"
        );
        assert_eq!(s.fraction(ShieldArc::Aft), 0.0);
        assert!(
            s.suppressed(ShieldArc::Aft),
            "and one timer holds it down whichever side is asked about"
        );
    }

    /// The two-bank fit keeps its old behaviour exactly — pooling is opt-in, and
    /// the player's own hull still rewards presenting a side.
    #[test]
    fn pooling_is_opt_in() {
        let mut s = fitted();
        assert!(!s.pooled, "the default fit is two banks");
        s.absorb(ShieldArc::Fore, 40.0);
        assert_eq!(s.fore.charge, 0.0);
        assert_eq!(s.aft.charge, 40.0, "the far side is untouched, as before");
        assert!(!s.suppressed(ShieldArc::Aft));
    }

    /// A capacity left on the stern of a pooled fit would be a pool nothing can
    /// reach — a design-panel toggle must not be able to create one.
    #[test]
    fn a_pooled_refit_clears_the_stern_capacity() {
        let s = Shield {
            pooled: true,
            fore_max: 100.0,
            aft_max: 60.0, // authored by mistake, or left over from a toggle
            ..Default::default()
        }
        .charged();
        let mut world = World::new();
        let e = world.spawn(s).id();
        let mut schedule = Schedule::default();
        schedule.add_systems(shield_refit_system);
        schedule.run(&mut world);
        let s = world.get::<Shield>(e).unwrap();
        assert_eq!(s.aft_max, 0.0);
        assert_eq!(s.aft.charge, 0.0);
        assert_eq!(s.fore.charge, 100.0, "the pool itself is untouched");
    }

    #[test]
    fn a_refit_never_leaves_an_over_full_bank() {
        let mut s = fitted();
        s.fore_max = 10.0; // the designer dragged capacity down mid-fight
        s.aft_max = 10.0;
        let mut world = World::new();
        let e = world.spawn(s).id();
        let mut schedule = Schedule::default();
        schedule.add_systems(shield_refit_system);
        schedule.run(&mut world);
        let s = world.get::<Shield>(e).unwrap();
        assert_eq!(s.fore.charge, 10.0);
        assert_eq!(s.aft.charge, 10.0);
    }
}
