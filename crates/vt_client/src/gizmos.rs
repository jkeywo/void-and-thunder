//! Gizmo overlays drawn directly onto the plane: the reference grid and system
//! boundary, aim beams/reticle/telegraphs, the boarding prompt, and the
//! microwarp range ring + destination ghost.

use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, TAU};
use vt_sim::prelude::*;

use crate::input::Aiming;
use crate::Player;

/// Spacing between grid lines on the plane.
const GRID_SPACING: f32 = 200.0;
/// Grid cells each way.
const GRID_CELLS: u32 = 30;
/// The grid sits just below the plane so hulls float above it.
const GRID_Z: f32 = -9.0;
/// Length of the drawn aim beam while holding a broadside.
const AIM_BEAM_LEN: f32 = 620.0;

/// The translucent preview of where a microwarp will drop the player.
#[derive(Component)]
pub struct MicrowarpGhost;

/// Draw the reference grid on the plane, plus the system boundary ring.
pub fn draw_grid(mut gizmos: Gizmos, bounds: Res<SystemBounds>) {
    gizmos.grid(
        Isometry3d::from_translation(Vec3::new(0.0, 0.0, GRID_Z)),
        UVec2::splat(GRID_CELLS),
        Vec2::splat(GRID_SPACING),
        Color::srgba(0.30, 0.38, 0.55, 0.28),
    );

    // The soft boundary of the star system, as a ring of segments.
    let segments = 96;
    let color = Color::srgba(0.55, 0.35, 0.35, 0.35);
    for i in 0..segments {
        let a0 = (i as f32 / segments as f32) * TAU;
        let a1 = ((i + 1) as f32 / segments as f32) * TAU;
        let p0 = Vec2::from_angle(a0) * bounds.radius;
        let p1 = Vec2::from_angle(a1) * bounds.radius;
        gizmos.line(p0.extend(GRID_Z), p1.extend(GRID_Z), color);
    }
}

/// While a broadside is held, draw where its volley would go — one beam per
/// gun, using the sim's own volley geometry so the preview cannot drift from
/// what actually fires. The beams glow **amber when the side is loaded** and go
/// **dim red while it's still reloading**, so you can see at a glance whether a
/// release will actually fire.
pub fn draw_aim_beams(
    mut gizmos: Gizmos,
    aiming: Res<Aiming>,
    player: Query<(&Transform, &Heading, &Velocity, &Broadside, &PilotIntent), With<Player>>,
) {
    let Ok((transform, heading, velocity, bank, pilot)) = player.single() else {
        return;
    };
    let pos = transform.translation.truncate();
    let aim_dir = (pilot.aim_point - pos).normalize_or_zero();

    for (active, is_port) in [(aiming.port, true), (aiming.starboard, false)] {
        if !active {
            continue;
        }
        let color = if bank.ready(is_port) {
            Color::srgba(1.0, 0.85, 0.4, 0.6) // loaded — amber
        } else {
            Color::srgba(1.0, 0.30, 0.25, 0.35) // reloading — dim red
        };
        let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
        for shot in broadside_volley(pos, velocity.0, dir, bank) {
            let beam = shot.velocity.normalize_or_zero();
            let start = shot.position.extend(0.0);
            let end = (shot.position + beam * AIM_BEAM_LEN).extend(0.0);
            gizmos.line(start, end, color);
        }
    }
}

/// Draw the boarding prompt: a ring around the crippled ship you're holding
/// alongside, filling clockwise as the dwell completes. It appears only while
/// the protagonist is in range (the sim sets [`Boarding::target`] then), giving
/// the "close enough to board" cue.
pub fn draw_boarding(mut gizmos: Gizmos, boarding: Res<Boarding>, ships: Query<&Transform>) {
    let Some(target) = boarding.target else {
        return;
    };
    let Ok(tf) = ships.get(target) else {
        return;
    };
    let pos = tf.translation.truncate();
    let frac = (boarding.progress / BOARD_DWELL).clamp(0.0, 1.0);
    let r = 46.0;
    // Dim base ring the moment you're in range.
    gizmos.circle(
        Isometry3d::from_translation(pos.extend(5.0)),
        r,
        Color::srgba(0.4, 0.9, 0.7, 0.4),
    );
    // Bright progress arc, sweeping clockwise from the top.
    let segs = 48usize;
    let filled = (frac * segs as f32).round() as usize;
    for i in 0..filled {
        let a0 = FRAC_PI_2 - (i as f32 / segs as f32) * TAU;
        let a1 = FRAC_PI_2 - ((i + 1) as f32 / segs as f32) * TAU;
        let p0 = pos + Vec2::from_angle(a0) * r;
        let p1 = pos + Vec2::from_angle(a1) * r;
        gizmos.line(
            p0.extend(5.0),
            p1.extend(5.0),
            Color::srgba(0.5, 1.0, 0.8, 0.95),
        );
    }
}

/// Draw a small reticle on the plane where the aim cursor points.
pub fn draw_reticle(mut gizmos: Gizmos, player: Query<&PilotIntent, With<Player>>) {
    let Ok(pilot) = player.single() else {
        return;
    };
    gizmos.circle(
        Isometry3d::from_translation(pilot.aim_point.extend(1.0)),
        14.0,
        Color::srgba(1.0, 1.0, 1.0, 0.5),
    );
}

/// While placing a microwarp, draw the reachable range ring and a line to the
/// clamped destination.
pub fn draw_microwarp_range(
    mut gizmos: Gizmos,
    player: Query<(&Transform, &MicrowarpDrive, &PilotIntent), With<Player>>,
) {
    let Ok((transform, drive, pilot)) = player.single() else {
        return;
    };
    if !pilot.microwarp_hold {
        return;
    }
    let ship = transform.translation.truncate();
    let dest = clamp_to_range(ship, pilot.aim_point, drive.range);
    let color = Color::srgba(0.4, 0.9, 0.7, 0.5);
    gizmos.circle(
        Isometry3d::from_translation(ship.extend(1.0)),
        drive.range,
        color,
    );
    gizmos.line(ship.extend(1.0), dest.extend(1.0), color);
}

/// Show the microwarp ghost at the clamped destination while the pilot aims a
/// warp, matching the player's heading; hide it otherwise.
pub fn microwarp_ghost(
    player: Query<
        (&Transform, &Heading, &MicrowarpDrive, &PilotIntent),
        (With<Player>, Without<MicrowarpGhost>),
    >,
    mut ghost: Query<(&mut Transform, &mut Visibility), With<MicrowarpGhost>>,
) {
    let Ok((mut ghost_tf, mut visibility)) = ghost.single_mut() else {
        return;
    };
    if let Ok((transform, heading, drive, pilot)) = player.single() {
        if pilot.microwarp_hold {
            let origin = transform.translation.truncate();
            let dest = clamp_to_range(origin, pilot.aim_point, drive.range);
            ghost_tf.translation = dest.extend(0.0);
            ghost_tf.rotation = Quat::from_rotation_z(heading.0);
            *visibility = Visibility::Visible;
            return;
        }
    }
    *visibility = Visibility::Hidden;
}

/// Draw the enemy fire telegraph: a red ring that closes in as a charging
/// broadside nears firing, plus a line along where the volley will go. Each side
/// charges independently, so both banks are drawn.
pub fn draw_charge_telegraph(mut gizmos: Gizmos, ships: Query<(&Transform, &Broadside)>) {
    for (transform, bank) in &ships {
        if bank.charge_time <= 0.0 {
            continue;
        }
        let pos = transform.translation.truncate();
        for state in [bank.port, bank.starboard] {
            if state.charging <= 0.0 {
                continue;
            }
            // 1 at the start of the wind-up, 0 the instant it fires.
            let t = (state.charging / bank.charge_time).clamp(0.0, 1.0);
            let radius = 20.0 + t * 70.0;
            let color = Color::srgba(1.0, 0.3, 0.25, 0.85);
            gizmos.circle(Isometry3d::from_translation(pos.extend(3.0)), radius, color);
            let end = pos + state.charge_dir.normalize_or_zero() * 130.0;
            gizmos.line(pos.extend(3.0), end.extend(3.0), color);
        }
    }
}
