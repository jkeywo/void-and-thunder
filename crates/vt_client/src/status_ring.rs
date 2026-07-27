//! Status rings: each ship's condition, projected onto the plane beneath its
//! hull and drawn by the HTML HUD.
//!
//! Everything the player needs mid-manoeuvre is here, in the one place their
//! eyes already are. The HUD at the screen edge is for glancing at between
//! fights; during one, looking away from your own ship to read a bar in the
//! corner is exactly when you lose the ship.
//!
//! **The ring is HTML, not a gizmo.** It used to be drawn with Bevy gizmos
//! directly on the plane, which was simple but meant the design was Rust line
//! segments — impossible to restyle without a rebuild, and impossible to author
//! anywhere else. Instead this module does only the geometry: it projects each
//! ship onto the screen and hands the page a transform that maps a *flat,
//! authored* ring onto the ground plane in perspective. The page owns what the
//! ring looks like; this owns where it goes.
//!
//! ## The transform contract
//!
//! A ring is authored as a **200×200 box, centred at (100,100), outer radius
//! 100** — a plain flat circle, no perspective baked in. For each ship we emit
//! three normalised screen-space vectors:
//!
//! - `o` — where the ship's centre lands on screen
//! - `x` — where a world **+X** offset of one ring radius lands, relative to `o`
//! - `y` — the same for world **−Y**
//!
//! `y` is the image of *minus* world Y on purpose. SVG's y-axis points down the
//! page while the world's +Y points up the plane, so mapping them directly would
//! silently mirror the artwork — fine for a plain ring, wrong the moment anyone
//! adds a chevron or a letter. Negating it means a design drawn the right way up
//! in a vector editor lands the right way up on the deck.
//!
//! Ship-relative features (the shield banks, the gun arcs, the tube ticks) need
//! to know where the bow is, so each ring also carries `h`: the bow's direction
//! **in ring-local degrees**, already converted into the SVG's clockwise-from-
//! east convention. Nothing downstream has to think about handedness again.
//!
//! Those two vectors are the image of the ring's own axes under the camera, so
//! feeding them into a CSS `matrix()` lays the flat artwork onto the plane with
//! exactly the perspective the 3D view has. A circle becomes the correct
//! ellipse, and it stays correct as the camera pitches, orbits and zooms,
//! without the page knowing anything about cameras.
//!
//! Values are **fractions of the viewport**, not pixels, because the two HUD
//! transports do not agree on what a pixel is: the native Ultralight view is
//! sized in *physical* pixels while the web overlay is laid out in *CSS* pixels.
//! The page multiplies by whatever its own viewport is and both are right.
//!
//! ## Why this does not thrash the HUD texture
//!
//! On native the HUD is an Ultralight page rasterised into a Bevy texture, and
//! that copy is already gated on the surface's dirty bounds — it only costs
//! anything on a frame the page actually repainted. A ring that rewrote its
//! styles every frame would defeat that, so two things keep it honest: the
//! script is not evaluated at all unless the payload changed since last frame,
//! and the page compares each value before touching the DOM. A ship sitting
//! still with full shields repaints nothing.

use bevy::prelude::*;
use std::fmt::Write as _;
use vt_sim::prelude::*;

use crate::camera::MainCamera;
use crate::data::FeelTuning;
use crate::Player;

/// World radius the authored ring's outer edge maps to, for the player.
///
/// The hull collider is 26 units, so this clears it with room for the outer
/// bands to sit clear of the model.
const PLAYER_RING_RADIUS: f32 = 62.0;

/// The same for other ships. Smaller so the player's own ring is never confused
/// with a target's at a glance.
const ENEMY_RING_RADIUS: f32 = 46.0;

/// The per-frame ring payload, ready to hand to the page.
///
/// Separate from [`HudSnapshot`](crate::hud::HudSnapshot) because the two change
/// at completely different rates: the readouts change when something happens,
/// the ring transforms change whenever the ship or camera moves, which is most
/// frames. Pushing them together would mean the whole HUD snapshot churned every
/// frame and its own change-gating would never fire.
#[derive(Resource, Default)]
pub struct RingSnapshot {
    pub json: String,
    /// Bumped only when `json` actually differs, so a transport can skip the
    /// script evaluation entirely on an unchanged frame.
    pub seq: u64,
}

/// Project one world point to normalised viewport coordinates (x right, y down,
/// both `0..1` across the visible area).
///
/// `None` when the point is not in front of the camera — Bevy's NDC depth is
/// reverse-Z, so anything outside `0..=1` is behind the eye or beyond the far
/// plane and must not be drawn at a mirrored position on screen.
fn project(camera: &Camera, camera_tf: &GlobalTransform, world: Vec3) -> Option<Vec2> {
    let ndc = camera.world_to_ndc(camera_tf, world)?;
    if !(0.0..=1.0).contains(&ndc.z) || !ndc.is_finite() {
        return None;
    }
    Some(Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5))
}

/// Build the ring payload for every ship the camera can see.
pub fn gather_ring_state(
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    ships: Query<
        (
            Entity,
            &Transform,
            &Heading,
            &Hull,
            Option<&Shield>,
            Option<&Broadside>,
            Option<&TorpedoBay>,
            Has<Player>,
            Has<Disabled>,
        ),
        With<Ship>,
    >,
    feel: Res<FeelTuning>,
    mut snap: ResMut<RingSnapshot>,
) {
    let Ok((camera, camera_tf)) = camera.single() else {
        return;
    };

    let mut j = String::with_capacity(512);
    j.push('[');
    let mut first = true;

    let plane_z = -feel.rings.drop;
    for (entity, transform, heading, hull, shield, bank, tubes, is_player, crippled) in &ships {
        let radius = if is_player {
            PLAYER_RING_RADIUS
        } else {
            ENEMY_RING_RADIUS
        };
        let centre = transform.translation.truncate().extend(plane_z);

        // The ring's own axes, as the camera sees them. Projecting the offsets
        // rather than deriving them from the camera's pitch is what keeps the
        // ellipse exact: it inherits the real projection, including the
        // perspective divide, which a flat rotateX approximation would not.
        let (Some(o), Some(px), Some(py)) = (
            project(camera, camera_tf, centre),
            project(camera, camera_tf, centre + Vec3::X * radius),
            // Minus Y: see the handedness note in the module docs.
            project(camera, camera_tf, centre - Vec3::Y * radius),
        ) else {
            continue; // behind the camera, or otherwise unprojectable
        };
        let (ex, ey) = (px - o, py - o);

        if !first {
            j.push(',');
        }
        first = false;

        let hull_frac = (hull.current / hull.max).clamp(0.0, 1.0);
        // `id` is the entity index: stable for a ship's whole life, so the page
        // can keep one element per ship instead of rebuilding the set each frame.
        let _ = write!(
            j,
            "{{\"id\":{},\"me\":{},\"prize\":{},\"o\":[{:.5},{:.5}],\"x\":[{:.5},{:.5}],\"y\":[{:.5},{:.5}],\"h\":{:.2},\"hull\":{:.4}",
            entity.index(),
            is_player,
            // Crippled: a prize to be boarded, not a threat. The ring must stop
            // shouting red at something you are meant to sail up to and take.
            crippled,
            o.x,
            o.y,
            ex.x,
            ex.y,
            ey.x,
            ey.y,
            // Into the SVG's clockwise-from-east convention, so the page can use
            // it as a rotation without knowing which way the world's Y points.
            -heading.0.to_degrees(),
            hull_frac
        );

        // Shields, when the hull carries any. `fitted` is separate from the
        // charges because "none fitted" and "both flat" must not look alike.
        if let Some(shield) = shield.filter(|s| s.fitted()) {
            let _ = write!(
                j,
                ",\"shield\":{{\"fore\":{:.4},\"aft\":{:.4}}}",
                shield.fraction(ShieldArc::Fore),
                shield.fraction(ShieldArc::Aft)
            );
        }

        // The gun and tube bands are the player's alone. A wave of enemies each
        // drawing firing arcs and tube pips would bury the aim beams and lead
        // diamonds already competing for that space.
        if is_player {
            if let Some(bank) = bank {
                for (is_port, key) in [(true, "port"), (false, "stbd")] {
                    let side = bank.side(is_port);
                    // How far through the reload, so the page can sweep an arc
                    // rather than print a number.
                    let loaded = if bank.cooldown > 0.0 {
                        (1.0 - side.timer / bank.cooldown).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let _ = write!(
                        j,
                        ",\"{key}\":{{\"ready\":{},\"arc\":{:.4},\"loaded\":{loaded:.3}}}",
                        bank.ready(is_port),
                        bank.arc
                    );
                }
            }
            if let Some(tubes) = tubes {
                // `loaded` is fractional: the whole part is tubes ready, the
                // fraction is the one currently reloading.
                let ready = tubes.loaded.floor().max(0.0) as u32;
                let loading = tubes.loaded - tubes.loaded.floor();
                let _ = write!(
                    j,
                    ",\"tubes\":{{\"max\":{},\"ready\":{ready},\"loading\":{loading:.3}}}",
                    tubes.tubes_max
                );
            }
        }
        j.push('}');
    }
    j.push(']');

    // Only bump the sequence when something actually moved. This is what lets a
    // still scene skip the script evaluation, and with it the page repaint and
    // the texture upload behind it.
    if j != snap.json {
        snap.json = j;
        snap.seq = snap.seq.wrapping_add(1);
    }
}
