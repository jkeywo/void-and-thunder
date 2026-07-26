//! The status ring: a ship's condition drawn on the plane beneath its hull.
//!
//! Everything the player needs mid-manoeuvre is here, in the one place their
//! eyes already are. The HUD at the screen edge is for glancing at between
//! fights; during one, looking away from your own ship to read a bar in the
//! corner is exactly when you lose the ship. So the ring carries hull, shields,
//! broadside readiness *and their arcs*, and torpedo tubes, concentrically:
//!
//! ```text
//!   r=34  hull            a full circle, draining anticlockwise, amber -> red
//!   r=42  shields         two 180-deg arcs, fore and aft, bright with charge
//!   r=52  broadsides      the port and starboard firing arcs, lit when loaded
//!   r=60  torpedo tubes   one pip per tube, filled when that tube is ready
//! ```
//!
//! The shield and broadside rings **rotate with the hull**, because that is the
//! whole point of them: which way you are facing decides which bank takes the
//! next blow and where your guns can reach. The hull ring does not — it is a
//! gauge, not a direction.
//!
//! Enemies get a reduced ring (hull, and shields if fitted). A wave of four
//! ships each drawing gun arcs and tube pips would bury the aim beams and lead
//! diamonds that are already competing for the same space.

use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, TAU};
use vt_sim::prelude::*;

use crate::Player;

/// Radii of the concentric bands, outward from the hull. The player's collider
/// is 26 units, so the innermost band clears it.
const R_HULL: f32 = 34.0;
const R_SHIELD: f32 = 42.0;
const R_BROADSIDE: f32 = 52.0;
const R_TUBES: f32 = 60.0;

/// Just under the hull and well above the grid at -9, so the ring reads as
/// painted on the deck beneath the ship rather than floating around it.
const RING_Z: f32 = -4.0;

/// Segments per full turn. Enough that a 180° arc is smooth at the radii here.
const SEGMENTS: usize = 72;

/// Draw an arc on the plane, centred on `centre`, from `from` to `to` radians.
///
/// Handles either winding: the caller says where the arc starts and ends, and
/// the segment count follows the span so a short arc is not oversampled.
fn arc(gizmos: &mut Gizmos, centre: Vec2, r: f32, from: f32, to: f32, color: Color) {
    let span = to - from;
    if span.abs() < 1e-4 || r <= 0.0 {
        return;
    }
    let steps = ((span.abs() / TAU) * SEGMENTS as f32).ceil().max(1.0) as usize;
    for i in 0..steps {
        let a0 = from + span * (i as f32 / steps as f32);
        let a1 = from + span * ((i + 1) as f32 / steps as f32);
        let p0 = centre + Vec2::from_angle(a0) * r;
        let p1 = centre + Vec2::from_angle(a1) * r;
        gizmos.line(p0.extend(RING_Z), p1.extend(RING_Z), color);
    }
}

/// A short radial tick, for marking the ends of an arc.
fn tick(gizmos: &mut Gizmos, centre: Vec2, angle: f32, inner: f32, outer: f32, color: Color) {
    let dir = Vec2::from_angle(angle);
    gizmos.line(
        (centre + dir * inner).extend(RING_Z),
        (centre + dir * outer).extend(RING_Z),
        color,
    );
}

/// Hull colour: amber while healthy, through orange, to red as it fails. The
/// same reading the HUD bar gives, so the two never disagree.
fn hull_color(frac: f32, alpha: f32) -> Color {
    let g = 0.20 + 0.55 * frac;
    Color::srgba(1.0, g, 0.18, alpha)
}

/// The hull band: a full ring showing what is left, drained anticlockwise from
/// the top so it empties the way a clock unwinds.
fn draw_hull_band(gizmos: &mut Gizmos, pos: Vec2, frac: f32) {
    // The empty remainder, so the ring is always a complete circle and the
    // missing portion reads as damage rather than as nothing being drawn.
    arc(
        gizmos,
        pos,
        R_HULL,
        FRAC_PI_2,
        FRAC_PI_2 + TAU,
        Color::srgba(0.35, 0.30, 0.30, 0.22),
    );
    arc(
        gizmos,
        pos,
        R_HULL,
        FRAC_PI_2,
        FRAC_PI_2 + TAU * frac,
        hull_color(frac, 0.95),
    );
}

/// The shield band: fore and aft half-circles that brighten with charge.
///
/// Both arcs are always drawn, faintly, even at zero — a bank you cannot see is
/// a bank you forget you have, and "my stern is bare" is the single most useful
/// thing this ring tells you.
fn draw_shield_band(gizmos: &mut Gizmos, pos: Vec2, heading: f32, shield: &Shield) {
    if !shield.fitted() {
        return;
    }
    for (arc_id, centre_angle) in [
        (ShieldArc::Fore, heading),
        (ShieldArc::Aft, heading + std::f32::consts::PI),
    ] {
        let frac = shield.fraction(arc_id);
        let from = centre_angle - FRAC_PI_2;
        let to = centre_angle + FRAC_PI_2;
        // The empty half, always present.
        arc(
            gizmos,
            pos,
            R_SHIELD,
            from,
            to,
            Color::srgba(0.30, 0.55, 0.80, 0.20),
        );
        if frac <= 0.0 {
            continue;
        }
        // The charge grows from the middle of the arc outward to both ends, so a
        // half-full bank reads as "covering the centre of that side" rather than
        // as an arbitrary slice.
        let half = FRAC_PI_2 * frac;
        arc(
            gizmos,
            pos,
            R_SHIELD,
            centre_angle - half,
            centre_angle + half,
            Color::srgba(0.55, 0.85, 1.0, 0.35 + 0.6 * frac),
        );
    }
    // Ticks on the beam mark where one bank ends and the other begins.
    for side in [FRAC_PI_2, -FRAC_PI_2] {
        tick(
            gizmos,
            pos,
            heading + side,
            R_SHIELD - 3.0,
            R_SHIELD + 3.0,
            Color::srgba(0.55, 0.85, 1.0, 0.5),
        );
    }
}

/// The broadside band: each bank's actual firing arc, lit when it can fire.
///
/// The arc drawn is the sim's own `Broadside::arc` about the beam, so what you
/// see is exactly where a volley could be thrown — the same geometry the aim
/// beams use, at a glance and without having to hold the button down.
fn draw_broadside_band(gizmos: &mut Gizmos, pos: Vec2, heading: f32, bank: &Broadside) {
    for (is_port, side) in [(true, FRAC_PI_2), (false, -FRAC_PI_2)] {
        let beam = heading + side;
        let ready = bank.ready(is_port);
        let color = if ready {
            Color::srgba(1.0, 0.72, 0.25, 0.85)
        } else {
            // Dim red while reloading — the same language the aim beams use.
            Color::srgba(0.7, 0.25, 0.20, 0.45)
        };
        arc(
            gizmos,
            pos,
            R_BROADSIDE,
            beam - bank.arc,
            beam + bank.arc,
            color,
        );
        // End ticks, so the edge of the arc is unambiguous where it meets the
        // dark plane rather than just fading out.
        for end in [beam - bank.arc, beam + bank.arc] {
            tick(
                gizmos,
                pos,
                end,
                R_BROADSIDE - 4.0,
                R_BROADSIDE + 4.0,
                color,
            );
        }

        // A reloading bank sweeps a filler arc from the beam outward as it
        // recharges, so the wait is legible without reading a number.
        if !ready && bank.cooldown > 0.0 {
            let remaining = bank.side(is_port).timer / bank.cooldown;
            let done = (1.0 - remaining).clamp(0.0, 1.0);
            arc(
                gizmos,
                pos,
                R_BROADSIDE,
                beam - bank.arc * done,
                beam + bank.arc * done,
                Color::srgba(1.0, 0.72, 0.25, 0.5),
            );
        }
    }
}

/// The torpedo band: one pip per tube around the bow, filled when loaded.
///
/// Spread across the forward arc rather than the whole circle because that is
/// where torpedoes go, and because the after half of the ring belongs to the
/// aft shield.
fn draw_tube_band(gizmos: &mut Gizmos, pos: Vec2, heading: f32, bay: &TorpedoBay) {
    let tubes = bay.tubes_max;
    if tubes == 0 {
        return;
    }
    // `loaded` is fractional: the whole part is tubes ready, the fraction is the
    // one currently reloading. Same reading the HUD tube row gives.
    let ready = bay.loaded.floor().max(0.0) as u32;
    let loading = bay.loaded - bay.loaded.floor();

    let spread = TAU * 0.42; // a little under half the circle, centred on the bow
    for i in 0..tubes {
        // Single-tube bays sit dead ahead instead of dividing by zero.
        let t = if tubes == 1 {
            0.5
        } else {
            i as f32 / (tubes - 1) as f32
        };
        let angle = heading - spread * 0.5 + spread * t;
        let (inner, outer, color) = if i < ready {
            (
                R_TUBES - 4.0,
                R_TUBES + 4.0,
                Color::srgba(0.55, 1.0, 0.75, 0.9),
            )
        } else if i == ready && loading > 0.0 {
            // The tube mid-reload grows from nothing to full length.
            (
                R_TUBES - 4.0,
                R_TUBES - 4.0 + 8.0 * loading,
                Color::srgba(0.55, 1.0, 0.75, 0.5),
            )
        } else {
            (
                R_TUBES - 1.5,
                R_TUBES + 1.5,
                Color::srgba(0.35, 0.5, 0.42, 0.35),
            )
        };
        tick(gizmos, pos, angle, inner, outer, color);
    }
}

/// Draw the player's full status ring.
pub fn draw_player_status_ring(
    mut gizmos: Gizmos,
    player: Query<
        (
            &Transform,
            &Heading,
            &Hull,
            Option<&Shield>,
            Option<&Broadside>,
            Option<&TorpedoBay>,
        ),
        With<Player>,
    >,
) {
    let Ok((transform, heading, hull, shield, bank, tubes)) = player.single() else {
        return;
    };
    let pos = transform.translation.truncate();
    let frac = (hull.current / hull.max).clamp(0.0, 1.0);

    draw_hull_band(&mut gizmos, pos, frac);
    if let Some(shield) = shield {
        draw_shield_band(&mut gizmos, pos, heading.0, shield);
    }
    if let Some(bank) = bank {
        draw_broadside_band(&mut gizmos, pos, heading.0, bank);
    }
    if let Some(tubes) = tubes {
        draw_tube_band(&mut gizmos, pos, heading.0, tubes);
    }
}

/// Draw the reduced ring on every ship that is not the player: hull, and shield
/// arcs if the hull carries any.
///
/// Enough to pick a target and to see which side of it is soft, without the
/// gun-arc and tube clutter that would make a four-ship wave unreadable. Drawn
/// smaller than the player's so the two are never confused at a glance.
pub fn draw_enemy_status_rings(
    mut gizmos: Gizmos,
    ships: Query<(&Transform, &Heading, &Hull, Option<&Shield>), (With<Ship>, Without<Player>)>,
) {
    for (transform, heading, hull, shield) in &ships {
        let pos = transform.translation.truncate();
        let frac = (hull.current / hull.max).clamp(0.0, 1.0);

        // A single arc rather than the player's ring-plus-remainder: at this
        // size the empty portion just reads as noise.
        arc(
            &mut gizmos,
            pos,
            R_HULL * 0.85,
            FRAC_PI_2,
            FRAC_PI_2 + TAU * frac,
            hull_color(frac, 0.55),
        );

        let Some(shield) = shield else { continue };
        if !shield.fitted() {
            continue;
        }
        for (arc_id, centre_angle) in [
            (ShieldArc::Fore, heading.0),
            (ShieldArc::Aft, heading.0 + std::f32::consts::PI),
        ] {
            let charge = shield.fraction(arc_id);
            if charge <= 0.0 {
                continue;
            }
            let half = FRAC_PI_2 * charge;
            arc(
                &mut gizmos,
                pos,
                R_SHIELD * 0.85,
                centre_angle - half,
                centre_angle + half,
                Color::srgba(0.55, 0.85, 1.0, 0.20 + 0.4 * charge),
            );
        }
    }
}
