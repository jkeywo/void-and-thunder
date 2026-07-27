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
/// Half-diagonal of the lead diamond marking a predicted intercept.
const LEAD_MARKER_R: f32 = 20.0;

/// How far a bank's volley actually reaches: muzzle speed × how long a
/// cannonball lives before it falls into the void. The drawn aim beam runs
/// exactly this far, so the beam ends where the shots die rather than at an
/// arbitrary length that has to be re-tuned whenever the guns change.
fn bank_reach(bank: &Broadside, projectile_ttl: f32) -> f32 {
    bank.muzzle_speed * projectile_ttl
}

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
    tuning: Res<SimTuning>,
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
        for shot in broadside_volley(
            pos,
            velocity.0,
            dir,
            bank,
            tuning.hull_length,
            tuning.muzzle_standoff,
        ) {
            let beam = shot.velocity.normalize_or_zero();
            let start = shot.position.extend(0.0);
            let end = (shot.position + beam * bank_reach(bank, tuning.projectile_ttl)).extend(0.0);
            gizmos.line(start, end, color);
        }
    }
}

/// While a broadside is held, mark where each hostile that bank can bear on will
/// be *when the volley gets there* — the firing lead.
///
/// The marker sits on the target's predicted position rather than on some
/// offset aim point, because the aim beams already carry the ship's own
/// momentum: putting a beam through the diamond is the firing solution. A short
/// tail joins each target to its own mark, so the lead reads as "this much
/// ahead" rather than as a free-floating diamond.
///
/// Only targets this bank could actually swing onto are marked (the solution is
/// run through the sim's own arc clamp), and crippled hulks are left out — they
/// are prizes to board, not threats to shoot.
pub fn draw_aim_lead(
    mut gizmos: Gizmos,
    aiming: Res<Aiming>,
    tuning: Res<SimTuning>,
    player: Query<(&Transform, &Heading, &Velocity, &Broadside, &Faction), With<Player>>,
    targets: Query<
        (&Transform, &Velocity, &Faction),
        (With<Ship>, Without<Player>, Without<Disabled>),
    >,
) {
    let Ok((transform, heading, velocity, bank, faction)) = player.single() else {
        return;
    };
    let pos = transform.translation.truncate();

    for (active, is_port) in [(aiming.port, true), (aiming.starboard, false)] {
        if !active {
            continue;
        }
        // Match the beams: bright when this side can actually fire, dim while
        // it is still reloading.
        let (mark, tail) = if bank.ready(is_port) {
            (
                Color::srgba(1.0, 0.95, 0.7, 0.9),
                Color::srgba(1.0, 0.85, 0.4, 0.35),
            )
        } else {
            (
                Color::srgba(1.0, 0.45, 0.35, 0.5),
                Color::srgba(1.0, 0.30, 0.25, 0.2),
            )
        };

        for (target_tf, target_vel, target_faction) in &targets {
            if !faction.hostile_to(*target_faction) {
                continue;
            }
            let target = target_tf.translation.truncate();
            let Some(lead) =
                intercept_lead(pos, bank.muzzle_speed, velocity.0, target, target_vel.0)
            else {
                continue; // outrunning the guns
            };
            // A shot that would expire before arriving is out of range.
            if lead.time > tuning.projectile_ttl {
                continue;
            }
            // Where the volley would have to be thrown to make that intercept.
            // Only mark it if this bank can actually swing that far — checked by
            // running it through the sim's own clamp and seeing if it moved.
            let required =
                ((target - pos) + (target_vel.0 - velocity.0) * lead.time).normalize_or_zero();
            let clamped = broadside_direction(heading.0, is_port, Some(required), bank.arc);
            if clamped.dot(required) < 0.999 {
                continue;
            }

            gizmos.line(target.extend(2.0), lead.point.extend(2.0), tail);
            draw_diamond(&mut gizmos, lead.point, LEAD_MARKER_R, mark);
        }
    }
}

/// A diamond lying on the plane — the lead marker's shape, deliberately unlike
/// the rings used for torpedo locks and boarding.
fn draw_diamond(gizmos: &mut Gizmos, centre: Vec2, r: f32, color: Color) {
    let corners = [
        centre + Vec2::Y * r,
        centre + Vec2::X * r,
        centre - Vec2::Y * r,
        centre - Vec2::X * r,
    ];
    for i in 0..corners.len() {
        let a = corners[i];
        let b = corners[(i + 1) % corners.len()];
        gizmos.line(a.extend(2.0), b.extend(2.0), color);
    }
}

/// Draw the boarding prompt: a ring around the crippled ship you're holding
/// alongside, filling clockwise as the dwell completes. It appears only while
/// the protagonist is in range (the sim sets [`Boarding::target`] then), giving
/// the "close enough to board" cue.
pub fn draw_boarding(
    mut gizmos: Gizmos,
    boarding: Res<Boarding>,
    tuning: Res<SimTuning>,
    ships: Query<&Transform>,
) {
    let Some(target) = boarding.target else {
        return;
    };
    let Ok(tf) = ships.get(target) else {
        return;
    };
    let pos = tf.translation.truncate();
    let frac = (boarding.progress / tuning.board_dwell).clamp(0.0, 1.0);
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

/// Ring every ship the torpedo bay currently has locked, so the pilot can see
/// what a volley will actually chase before releasing. A target locked by more
/// than one tube gets a ring per lock, drawn progressively wider — that reads as
/// "two torpedoes are coming for this one".
pub fn draw_torpedo_locks(
    mut gizmos: Gizmos,
    player: Query<&TorpedoLock, With<Player>>,
    ships: Query<&Transform>,
) {
    let Ok(lock) = player.single() else {
        return;
    };
    // Count the locks per target so repeats nest instead of overdrawing.
    let mut seen: Vec<(Entity, u32)> = Vec::new();
    for target in lock.targets.iter().take(lock.locks as usize).flatten() {
        match seen.iter_mut().find(|(e, _)| e == target) {
            Some((_, n)) => *n += 1,
            None => seen.push((*target, 1)),
        }
    }

    let color = Color::srgba(1.0, 0.55, 0.25, 0.9);
    for (target, count) in seen {
        let Ok(tf) = ships.get(target) else {
            continue; // locked ship died this frame
        };
        let pos = tf.translation.truncate();
        for i in 0..count {
            let r = 38.0 + i as f32 * 7.0;
            // A dashed ring: eight ticks, so it reads as a lock rather than a hull.
            let segs = 8;
            for s in 0..segs {
                let a0 = (s as f32 / segs as f32) * TAU;
                let a1 = a0 + TAU / (segs as f32 * 2.0);
                gizmos.line(
                    (pos + Vec2::from_angle(a0) * r).extend(5.0),
                    (pos + Vec2::from_angle(a1) * r).extend(5.0),
                    color,
                );
            }
        }
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

/// While aiming a torpedo volley, draw the bay's reach and the lock brush under
/// the cursor.
///
/// The reach ring is deliberately the same circle the microwarp draws — both are
/// [`ENGAGEMENT_RANGE`] — so the two top-down tools read as covering the same
/// ground rather than each having their own invisible envelope.
pub fn draw_torpedo_range(
    mut gizmos: Gizmos,
    player: Query<(&Transform, &TorpedoBay, &PilotIntent), With<Player>>,
) {
    let Ok((transform, bay, pilot)) = player.single() else {
        return;
    };
    if !pilot.torpedo_hold {
        return;
    }
    let ship = transform.translation.truncate();
    gizmos.circle(
        Isometry3d::from_translation(ship.extend(1.0)),
        bay.range,
        Color::srgba(1.0, 0.55, 0.25, 0.35),
    );
    // The brush the cursor sweeps: anything inside it can be locked.
    gizmos.circle(
        Isometry3d::from_translation(pilot.aim_point.extend(1.0)),
        bay.lock_radius,
        Color::srgba(1.0, 0.65, 0.35, 0.45),
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

/// Ring the point-defence screen while it is up.
///
/// The whole device is otherwise invisible — shots simply stop arriving — so the
/// ring is what tells the player where the bubble ends and why that torpedo did
/// not land. Drawn only while held, because an always-on circle around your own
/// ship is furniture rather than information.
pub fn draw_point_defense_radius(
    mut gizmos: Gizmos,
    player: Query<(&Transform, &PointDefense), With<Player>>,
) {
    let Ok((transform, screen)) = player.single() else {
        return;
    };
    if !screen.powered {
        return;
    }
    let ship = transform.translation.truncate();
    gizmos.circle(
        Isometry3d::from_translation(ship.extend(1.0)),
        screen.radius,
        Color::srgba(0.55, 0.85, 1.0, 0.45),
    );
}

/// One line from the emitter to something it just swatted, fading out.
///
/// How long a flash stays up. Short enough to read as a *snap* rather than a
/// beam — the screen fires several times a second, and a long line would smear
/// into a permanent cone.
const INTERCEPT_FLASH_LIFE: f32 = 0.14;

/// A kill the screen has made and not yet finished drawing.
///
/// Public only because it appears in the system's `Local` parameter, and Bevy
/// requires every system parameter type to be at least as visible as the system.
pub struct InterceptFlash {
    from: Vec2,
    /// Full 3D: a torpedo dies well off the plane, and a line drawn to its
    /// flattened shadow would miss the thing the player watched vanish.
    to: Vec3,
    age: f32,
}

/// Flash a line from the point-defence emitter to each munition it takes.
///
/// Without this the screen is invisible — shots simply stop arriving, which
/// reads as luck rather than as the device the player fitted doing its job.
///
/// The flashes are held in a `Local` rather than spawned as entities: they are
/// two points and a clock, they never interact with anything, and the sim
/// already told us where and when. Aged on virtual time, so they stretch under
/// bullet-time along with everything else and stop dead on a pause.
pub fn draw_intercept_flashes(
    mut gizmos: Gizmos,
    time: Res<Time>,
    mut intercepts: MessageReader<MunitionIntercepted>,
    mut flashes: Local<Vec<InterceptFlash>>,
) {
    for intercepted in intercepts.read() {
        flashes.push(InterceptFlash {
            from: intercepted.from,
            to: intercepted.position,
            age: 0.0,
        });
    }

    let dt = time.delta_secs();
    flashes.retain_mut(|flash| {
        flash.age += dt;
        let t = flash.age / INTERCEPT_FLASH_LIFE;
        if t >= 1.0 {
            return false;
        }
        // Bright and hot at the moment of the kill, gone almost at once.
        let color = Color::srgba(0.8, 0.95, 1.0, 1.0 - t);
        gizmos.line(flash.from.extend(1.0), flash.to, color);
        true
    });
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
