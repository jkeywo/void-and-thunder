//! Hull-to-hull collision: shoving, and ramming.
//!
//! Two ships used to pass straight through one another. That is the cheapest
//! possible way to make a fleet feel weightless — nothing in the world pushes
//! back, so the hull may as well be a cursor. Rebel Galaxy's own postmortem put
//! it plainly: ships should feel like "Tonka trucks in space", and *"if they
//! didn't bang around and smash into things, they wouldn't feel as good."*
//!
//! Every contact does three things:
//!
//! 1. **Separates** the pair, so hulls never sit inside each other.
//! 2. **Exchanges momentum** along the contact normal, with some restitution, so
//!    a shove reads as a shove. Only the closing component is touched — the
//!    tangential slide is left alone, so ships scrape past rather than sticking.
//! 3. **Hurts, above a threshold.** Drifting into someone is free; driving your
//!    bow into them at speed is a weapon. Damage goes through
//!    [`apply_hull_damage`] like every other source, so brace and invulnerability
//!    behave exactly as they do against a broadside.
//!
//! It emits [`ShipHit`] at the contact point, which means the whole existing
//! feedback chain — trauma, camera kick, hitstop, sparks, the hit sound — fires
//! for a ram without the client needing to know rams exist.
//!
//! Pairs are resolved in one naive O(n²) pass. The field holds a handful of
//! ships, and the projectile hit test next door is already O(projectiles ×
//! ships); a broadphase here would be structure for its own sake.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_transform::components::Transform;

use crate::combat::apply_hull_damage;
use crate::components::{
    Anchored, Brace, Collider, Faction, Heading, Hull, Invulnerable, Ship, Velocity,
};
use crate::drive::BoostDrive;
use crate::events::ShipHit;
use crate::shield::{shield_arc, Shield};
use crate::tuning::SimTuning;

/// How much of the closing speed is given back as bounce. Well under 1: hulls
/// are not billiard balls, and a lively bounce reads as a bug rather than as
/// mass. The live value is [`SimTuning::ram_restitution`].
pub const RAM_RESTITUTION: f32 = 0.35;
/// Closing speed (units/s) below which a contact is a nudge and costs nothing.
/// Without a floor, ships jostling in a scrum would grind each other down.
pub const RAM_DAMAGE_THRESHOLD: f32 = 45.0;
/// Hull damage per unit of closing speed *above* the threshold.
pub const RAM_DAMAGE_PER_SPEED: f32 = 0.22;
/// How much harder a boosted, bow-on ram lands, at full alignment.
///
/// The drive is already a commitment — a spent battery and a wide turn — so
/// spending it to put your bow through someone should be a real attack rather
/// than a slightly firmer nudge. Scaled by how squarely the bow meets them, so
/// clipping a hull in passing while boosting gains almost nothing.
pub const RAM_BOOST_BONUS: f32 = 2.5;
/// Fraction of the overlap corrected per step. Below 1 so a deep overlap eases
/// apart over a few frames instead of teleporting, which would fight the
/// client's pose interpolation and read as a flicker.
pub const RAM_SEPARATION: f32 = 0.6;

/// Damage a contact does at `closing_speed`. Pure, so the curve can be asserted
/// on directly, and so the threshold's behaviour is pinned by a test rather than
/// by playtesting.
pub fn ram_damage(closing_speed: f32, threshold: f32, per_speed: f32) -> f32 {
    (closing_speed - threshold).max(0.0) * per_speed
}

/// The multiplier a rammer's blow gets for driving its bow into the contact
/// under boost.
///
/// `bow_on` is how squarely the bow meets the other hull: `1.0` dead-on, `0.0`
/// or less abeam or astern. A ship that is not boosting gets nothing, however
/// well aimed — the bonus is for spending the drive, not for arriving.
pub fn ram_boost_multiplier(boosting: bool, bow_on: f32, bonus: f32) -> f32 {
    if !boosting {
        return 1.0;
    }
    1.0 + bonus * bow_on.clamp(0.0, 1.0)
}

/// Bevy system: separate overlapping hulls, exchange momentum, and hurt the pair
/// if they met hard enough.
pub fn ram_system(
    tuning: Res<SimTuning>,
    mut hits: MessageWriter<ShipHit>,
    mut ships: Query<
        (
            Entity,
            &mut Transform,
            &Heading,
            &mut Velocity,
            &Collider,
            &Faction,
            &mut Hull,
            Option<&mut Shield>,
            Option<&BoostDrive>,
            Option<&Brace>,
            Has<Invulnerable>,
            Has<Anchored>,
        ),
        With<Ship>,
    >,
) {
    // Bevy cannot hand out two mutable borrows into the same query, so collect
    // the entity list and step pairs through `get_many_mut`.
    let entities: Vec<Entity> = ships.iter().map(|(e, ..)| e).collect();

    for i in 0..entities.len() {
        for j in (i + 1)..entities.len() {
            let Ok([a, b]) = ships.get_many_mut([entities[i], entities[j]]) else {
                continue;
            };
            let (
                a_entity,
                mut a_tf,
                a_head,
                mut a_vel,
                a_col,
                a_fac,
                mut a_hull,
                a_shield,
                a_boost,
                a_brace,
                a_inv,
                a_anchored,
            ) = a;
            let (
                b_entity,
                mut b_tf,
                b_head,
                mut b_vel,
                b_col,
                b_fac,
                mut b_hull,
                b_shield,
                b_boost,
                b_brace,
                b_inv,
                b_anchored,
            ) = b;

            let a_pos = a_tf.translation.truncate();
            let b_pos = b_tf.translation.truncate();
            let delta = b_pos - a_pos;
            let reach = a_col.radius + b_col.radius;
            let dist_sq = delta.length_squared();
            if dist_sq >= reach * reach {
                continue;
            }

            // Exactly concentric: pick an arbitrary axis rather than dividing by
            // zero. Rare, but a spawn overlap would otherwise produce NaN
            // positions that quietly poison every downstream system.
            let (normal, dist) = if dist_sq > 1e-6 {
                let dist = dist_sq.sqrt();
                (delta / dist, dist)
            } else {
                (Vec2::X, 0.0)
            };

            // Closing speed along the contact normal. Positive means approaching;
            // a pair already separating is left alone so they cannot be caught in
            // a loop of being pushed apart and re-hit.
            let closing = (a_vel.0 - b_vel.0).dot(normal);

            // An anchored ship is immovable: it absorbs none of the correction
            // and none of the impulse, so the mover takes all of both.
            let (a_share, b_share) = match (a_anchored, b_anchored) {
                (true, true) => (0.0, 0.0),
                (true, false) => (0.0, 1.0),
                (false, true) => (1.0, 0.0),
                (false, false) => (0.5, 0.5),
            };

            let overlap = (reach - dist) * tuning.ram_separation;
            a_tf.translation -= (normal * overlap * a_share).extend(0.0);
            b_tf.translation += (normal * overlap * b_share).extend(0.0);

            if closing <= 0.0 {
                continue;
            }

            // Cancel the closing component and give a fraction of it back.
            let impulse = closing * (1.0 + tuning.ram_restitution);
            a_vel.0 -= normal * impulse * a_share;
            b_vel.0 += normal * impulse * b_share;

            let base = ram_damage(
                closing,
                tuning.ram_damage_threshold,
                tuning.ram_damage_per_speed,
            );
            if base <= 0.0 {
                continue;
            }

            // Who drove into whom. `normal` runs from A toward B, so A meets B
            // bow-on when A's bow points along it, and B when its bow points
            // back down it. The bonus lands on the blow the rammer *deals*, not
            // on what they take: putting your bow through someone under power
            // should hurt them, not you.
            let boosting = |b: Option<&BoostDrive>| b.is_some_and(|d| d.active);
            let a_bow_on = Vec2::from_angle(a_head.0).dot(normal);
            let b_bow_on = Vec2::from_angle(b_head.0).dot(-normal);
            let to_b =
                base * ram_boost_multiplier(boosting(a_boost), a_bow_on, tuning.ram_boost_bonus);
            let to_a =
                base * ram_boost_multiplier(boosting(b_boost), b_bow_on, tuning.ram_boost_bonus);

            // Both hulls take it: a ram is not free for the rammer. The contact
            // point sits on the line between the two centres, so each ship's arc
            // is decided by which of its own ends met the other — ramming bow-on
            // spends your fore shield, being rammed from astern spends your aft.
            let contact = a_pos + normal * a_col.radius;
            let a_report = apply_hull_damage(
                &mut a_hull,
                a_shield.map(Mut::into_inner),
                shield_arc(a_head.0, a_pos, contact),
                to_a,
                a_brace.is_some_and(|x| x.active),
                a_inv,
                tuning.brace_damage_factor,
            );
            let b_report = apply_hull_damage(
                &mut b_hull,
                b_shield.map(Mut::into_inner),
                shield_arc(b_head.0, b_pos, contact),
                to_b,
                b_brace.is_some_and(|x| x.active),
                b_inv,
                tuning.brace_damage_factor,
            );
            // Announced per hull, like a cannonball, so the client shakes and
            // sparks for whichever of the two the camera cares about.
            // The contact normal, pointing into each hull in turn, so the sparks
            // fly off the side that was actually struck.
            hits.write(ShipHit {
                position: contact,
                ship: a_entity,
                faction: *a_fac,
                damage: to_a,
                direction: -normal,
                report: a_report,
            });
            hits.write(ShipHit {
                position: contact,
                ship: b_entity,
                faction: *b_fac,
                damage: to_b,
                direction: normal,
                report: b_report,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gentle_nudge_is_free() {
        assert_eq!(
            ram_damage(10.0, RAM_DAMAGE_THRESHOLD, RAM_DAMAGE_PER_SPEED),
            0.0
        );
        assert_eq!(
            ram_damage(
                RAM_DAMAGE_THRESHOLD,
                RAM_DAMAGE_THRESHOLD,
                RAM_DAMAGE_PER_SPEED
            ),
            0.0,
            "exactly at the threshold is still a nudge"
        );
    }

    #[test]
    fn a_boosted_bow_on_ram_hits_much_harder() {
        let plain = ram_boost_multiplier(false, 1.0, RAM_BOOST_BONUS);
        let bow_on = ram_boost_multiplier(true, 1.0, RAM_BOOST_BONUS);
        assert_eq!(plain, 1.0, "no drive spent, no bonus — however well aimed");
        assert!(
            bow_on >= 3.0,
            "driving the bow home under boost should be a real attack, was {bow_on}x"
        );
        // Clipping someone in passing gains almost nothing, even under boost.
        let glancing = ram_boost_multiplier(true, 0.1, RAM_BOOST_BONUS);
        assert!(
            glancing < 1.5,
            "a glancing boosted contact should barely differ, was {glancing}x"
        );
        // Reversing into someone under boost is not a ram.
        assert_eq!(ram_boost_multiplier(true, -1.0, RAM_BOOST_BONUS), 1.0);
    }

    #[test]
    fn a_hard_ram_hurts_and_scales_with_speed() {
        let slow = ram_damage(80.0, RAM_DAMAGE_THRESHOLD, RAM_DAMAGE_PER_SPEED);
        let fast = ram_damage(160.0, RAM_DAMAGE_THRESHOLD, RAM_DAMAGE_PER_SPEED);
        assert!(slow > 0.0, "80 u/s should hurt, got {slow}");
        assert!(
            fast > slow * 2.0,
            "damage should climb faster than linearly off the threshold: {slow} then {fast}"
        );
    }
}
