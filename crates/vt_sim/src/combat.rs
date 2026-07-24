//! Combat: broadside firing, projectile flight, collision and damage.
//!
//! As with movement, the geometry and damage rules are pure functions so they
//! can be tested headlessly; the Bevy systems wrap them and touch the `World`.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{
    Brace, Broadside, Collider, Faction, FireOrders, Heading, Hull, Projectile, Ttl, Velocity,
};
use crate::events::{ShipDestroyed, ShipHit};

/// Fraction of damage a braced ship still takes (Black-Flag brace).
pub const BRACE_DAMAGE_FACTOR: f32 = 0.35;

/// How long a cannonball lives before falling into the void (seconds).
pub const PROJECTILE_TTL: f32 = 2.5;
/// Cannonball collision radius.
pub const PROJECTILE_RADIUS: f32 = 5.0;

/// One cannonball's spawn state: where it appears and how fast it travels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileSpawn {
    pub position: Vec2,
    pub velocity: Vec2,
}

/// The world direction a broadside on `heading` throws along, given the pilot's
/// desired `aim` (or `None` to fire straight out the beam), clamped to `±arc` of
/// the beam. `port` is the ship's left beam (+90° from bow), else starboard.
pub fn broadside_direction(heading: f32, port: bool, aim: Option<Vec2>, arc: f32) -> Vec2 {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let beam = if port {
        heading + FRAC_PI_2
    } else {
        heading - FRAC_PI_2
    };
    let wrap = |a: f32| {
        let a = a.rem_euclid(TAU);
        if a > PI {
            a - TAU
        } else {
            a
        }
    };
    let angle = match aim {
        Some(dir) if dir.length_squared() > 1e-6 => {
            let off = wrap(dir.to_angle() - beam).clamp(-arc, arc);
            beam + off
        }
        _ => beam,
    };
    Vec2::from_angle(angle)
}

/// Compute the volley of shots a broadside throws along `fire_dir` (a unit
/// world direction). Guns are spread along the hull (perpendicular to the fire
/// direction) so the volley leaves a line, not a point. Muzzle velocity is added
/// to the ship's own velocity — you inherit the ship's momentum.
pub fn broadside_volley(
    ship_pos: Vec2,
    ship_vel: Vec2,
    fire_dir: Vec2,
    bank: &Broadside,
) -> Vec<ProjectileSpawn> {
    let dir = fire_dir.normalize_or(Vec2::X);
    let along = dir.perp(); // spread the guns across the hull
    let guns = bank.guns.max(1);

    let hull_length = 40.0;
    let mut out = Vec::with_capacity(guns as usize);
    for i in 0..guns {
        let t = if guns == 1 {
            0.0
        } else {
            (i as f32 / (guns - 1) as f32) - 0.5
        };
        let muzzle = ship_pos + along * (t * hull_length) + dir * 22.0;
        let velocity = ship_vel + dir * bank.muzzle_speed;
        out.push(ProjectileSpawn {
            position: muzzle,
            velocity,
        });
    }
    out
}

/// Circle-vs-circle overlap test used for cannonball hits.
pub fn circles_overlap(a: Vec2, ra: f32, b: Vec2, rb: f32) -> bool {
    a.distance_squared(b) <= (ra + rb) * (ra + rb)
}

/// Bevy system: fire aimed broadsides, with a per-bank charge/telegraph.
///
/// [`FireOrders`] is a *request*, consumed here — raise a side and (if the bank
/// has reloaded) it begins a `charge_time` wind-up along the clamped aim
/// direction, then fires. `charge_time` 0 (the player) fires immediately; an
/// enemy's ~0.5s wind-up is drawn by the client so the shot is dodgeable. The AI
/// re-requests every tick, so it keeps firing on cooldown.
pub fn weapons_system(
    time: Res<Time>,
    mut commands: Commands,
    mut ships: Query<(
        &Transform,
        &Heading,
        &Velocity,
        &Faction,
        &mut Broadside,
        &mut FireOrders,
    )>,
) {
    let dt = time.delta_secs();
    for (transform, heading, velocity, faction, mut bank, mut orders) in &mut ships {
        let pos = transform.translation.truncate();
        // A config snapshot for the volley geometry, so we can freely mutate the
        // two banks' reload/charge state below without aliasing.
        let cfg = *bank;

        // Tick each side's reload independently.
        bank.port.timer = (bank.port.timer - dt).max(0.0);
        bank.starboard.timer = (bank.starboard.timer - dt).max(0.0);

        // Consume this step's request (a shot asked for mid-reload is dropped,
        // not queued). Both sides fire this step if both are ready and requested.
        let (want_port, want_starboard) = (orders.port, orders.starboard);
        let aim = orders.aim;
        orders.port = false;
        orders.starboard = false;
        orders.aim = None;

        for (port, want) in [(true, want_port), (false, want_starboard)] {
            // Resolve an in-progress charge on this side first.
            if bank.side(port).charging > 0.0 {
                let done = {
                    let s = bank.side_mut(port);
                    s.charging = (s.charging - dt).max(0.0);
                    s.charging <= 0.0
                };
                if done {
                    let dir = bank.side(port).charge_dir;
                    fire_broadside(&mut commands, pos, velocity.0, dir, &cfg, *faction);
                    bank.side_mut(port).timer = cfg.cooldown;
                }
                continue; // a charging side ignores fresh requests
            }

            if !want || bank.side(port).timer > 0.0 {
                continue;
            }
            let dir = broadside_direction(heading.0, port, aim, cfg.arc);
            if cfg.charge_time > 0.0 {
                let s = bank.side_mut(port);
                s.charging = cfg.charge_time;
                s.charge_dir = dir;
            } else {
                fire_broadside(&mut commands, pos, velocity.0, dir, &cfg, *faction);
                bank.side_mut(port).timer = cfg.cooldown;
            }
        }
    }
}

/// Spawn one broadside volley along `dir`.
fn fire_broadside(
    commands: &mut Commands,
    pos: Vec2,
    ship_vel: Vec2,
    dir: Vec2,
    bank: &Broadside,
    faction: Faction,
) {
    for shot in broadside_volley(pos, ship_vel, dir, bank) {
        commands.spawn((
            Projectile {
                damage: bank.damage,
                faction,
                radius: PROJECTILE_RADIUS,
            },
            Velocity(shot.velocity),
            Transform::from_translation(shot.position.extend(0.0)),
            Ttl(PROJECTILE_TTL),
        ));
    }
}

/// Bevy system: fly projectiles forward and expire them.
pub fn projectile_system(
    time: Res<Time>,
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Transform, &Velocity, &mut Ttl), With<Projectile>>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, velocity, mut ttl) in &mut projectiles {
        transform.translation += (velocity.0 * dt).extend(0.0);
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Bevy system: resolve cannonball hits against ships and apply damage,
/// announcing each hit for the presentation layer.
pub fn collision_system(
    mut commands: Commands,
    mut hits: MessageWriter<ShipHit>,
    projectiles: Query<(Entity, &Transform, &Projectile)>,
    mut ships: Query<(&Transform, &Collider, &Faction, &mut Hull, Option<&Brace>)>,
) {
    for (proj_entity, proj_tf, projectile) in &projectiles {
        let proj_pos = proj_tf.translation.truncate();
        for (ship_tf, collider, faction, mut hull, brace) in &mut ships {
            if !projectile.faction.hostile_to(*faction) {
                continue;
            }
            let ship_pos = ship_tf.translation.truncate();
            if circles_overlap(proj_pos, projectile.radius, ship_pos, collider.radius) {
                let braced = brace.is_some_and(|b| b.active);
                let damage = projectile.damage * if braced { BRACE_DAMAGE_FACTOR } else { 1.0 };
                hull.current -= damage;
                hits.write(ShipHit {
                    position: proj_pos,
                    faction: *faction,
                });
                commands.entity(proj_entity).despawn();
                break; // one ball, one hit
            }
        }
    }
}

/// Bevy system: remove ships whose hull has been reduced to zero, announcing
/// each destruction.
pub fn destruction_system(
    mut commands: Commands,
    mut destroyed: MessageWriter<ShipDestroyed>,
    ships: Query<(Entity, &Transform, &Faction, &Hull)>,
) {
    for (entity, transform, faction, hull) in &ships {
        if hull.current <= 0.0 {
            destroyed.write(ShipDestroyed {
                position: transform.translation.truncate(),
                faction: *faction,
            });
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volley_has_one_spawn_per_gun() {
        let bank = Broadside {
            guns: 3,
            ..Default::default()
        };
        let volley = broadside_volley(Vec2::ZERO, Vec2::ZERO, Vec2::Y, &bank);
        assert_eq!(volley.len(), 3);
    }

    #[test]
    fn beam_direction_defaults_to_the_side() {
        // Facing +X (heading 0): port beam is +Y, starboard is -Y.
        assert!(broadside_direction(0.0, true, None, 0.6).y > 0.9);
        assert!(broadside_direction(0.0, false, None, 0.6).y < -0.9);
    }

    #[test]
    fn aim_is_clamped_to_the_arc() {
        // Facing +X, port beam +Y (90°); asking to fire forward (+X, 0°) is 90°
        // off the beam but the arc is only ~34°, so it clamps toward +Y.
        let dir = broadside_direction(0.0, true, Some(Vec2::X), 0.6);
        let angle = dir.to_angle();
        assert!(
            angle >= std::f32::consts::FRAC_PI_2 - 0.6 - 1e-3,
            "clamped angle {angle} left the arc"
        );
    }

    #[test]
    fn shots_inherit_ship_momentum() {
        let bank = Broadside {
            guns: 1,
            ..Default::default()
        };
        // Fire straight +Y; the ship's +X momentum carries into the shot.
        let ship_vel = Vec2::new(50.0, 0.0);
        let shot = broadside_volley(Vec2::ZERO, ship_vel, Vec2::Y, &bank)[0];
        assert!(
            (shot.velocity.x - 50.0).abs() < 1e-4,
            "vx was {}",
            shot.velocity.x
        );
    }

    #[test]
    fn overlap_detects_a_hit() {
        assert!(circles_overlap(Vec2::ZERO, 5.0, Vec2::new(8.0, 0.0), 5.0));
        assert!(!circles_overlap(Vec2::ZERO, 5.0, Vec2::new(20.0, 0.0), 5.0));
    }

    #[test]
    fn friendly_fire_is_ignored() {
        assert!(!Faction::Corsairs.hostile_to(Faction::Corsairs));
        assert!(Faction::Corsairs.hostile_to(Faction::Houses));
        assert!(Faction::Houses.hostile_to(Faction::Freebooters));
    }
}
