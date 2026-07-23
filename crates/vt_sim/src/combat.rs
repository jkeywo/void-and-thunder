//! Combat: broadside firing, projectile flight, collision and damage.
//!
//! As with movement, the geometry and damage rules are pure functions so they
//! can be tested headlessly; the Bevy systems wrap them and touch the `World`.

use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_time::Time;
use bevy_transform::components::Transform;

use crate::components::{
    Broadside, Collider, Faction, FireOrders, Heading, Hull, Projectile, Ttl, Velocity,
};
use crate::events::{ShipDestroyed, ShipHit};

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

/// Compute the volley of cannonballs a broadside throws.
///
/// `port` fires out the ship's left (+90° from bow), `starboard` out the right.
/// Guns are spread evenly along the hull so the volley leaves a line, not a
/// point. Muzzle velocity is added to the ship's own velocity (you inherit the
/// ship's momentum), which is what makes leading a moving target feel right.
pub fn broadside_volley(
    ship_pos: Vec2,
    ship_vel: Vec2,
    heading: f32,
    bank: &Broadside,
    port: bool,
) -> Vec<ProjectileSpawn> {
    let forward = Vec2::from_angle(heading);
    // Left of the bow is a +90° rotation; right is -90°.
    let side = if port {
        forward.perp()
    } else {
        -forward.perp()
    };
    let guns = bank.guns.max(1);

    // Spread muzzle points along the hull, centred on the ship.
    let hull_length = 40.0;
    let mut out = Vec::with_capacity(guns as usize);
    for i in 0..guns {
        let t = if guns == 1 {
            0.0
        } else {
            (i as f32 / (guns - 1) as f32) - 0.5
        };
        let muzzle = ship_pos + forward * (t * hull_length) + side * 22.0;
        let velocity = ship_vel + side * bank.muzzle_speed;
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

/// Bevy system: tick broadside cooldowns and spawn cannonballs for any ship
/// with standing [`FireOrders`].
pub fn weapons_system(
    time: Res<Time>,
    mut commands: Commands,
    mut ships: Query<(
        &Transform,
        &Heading,
        &Velocity,
        &Faction,
        &mut Broadside,
        &FireOrders,
    )>,
) {
    let dt = time.delta_secs();
    for (transform, heading, velocity, faction, mut bank, orders) in &mut ships {
        if bank.timer > 0.0 {
            bank.timer = (bank.timer - dt).max(0.0);
        }
        if bank.timer > 0.0 || (!orders.port && !orders.starboard) {
            continue;
        }
        let pos = transform.translation.truncate();
        let mut fired = false;
        for &(side_active, is_port) in &[(orders.port, true), (orders.starboard, false)] {
            if !side_active {
                continue;
            }
            for shot in broadside_volley(pos, velocity.0, heading.0, &bank, is_port) {
                commands.spawn((
                    Projectile {
                        damage: bank.damage,
                        faction: *faction,
                        radius: PROJECTILE_RADIUS,
                    },
                    Velocity(shot.velocity),
                    Transform::from_translation(shot.position.extend(0.0)),
                    Ttl(PROJECTILE_TTL),
                ));
            }
            fired = true;
        }
        if fired {
            bank.timer = bank.cooldown;
        }
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
    mut ships: Query<(&Transform, &Collider, &Faction, &mut Hull)>,
) {
    for (proj_entity, proj_tf, projectile) in &projectiles {
        let proj_pos = proj_tf.translation.truncate();
        for (ship_tf, collider, faction, mut hull) in &mut ships {
            if !projectile.faction.hostile_to(*faction) {
                continue;
            }
            let ship_pos = ship_tf.translation.truncate();
            if circles_overlap(proj_pos, projectile.radius, ship_pos, collider.radius) {
                hull.current -= projectile.damage;
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
        let volley = broadside_volley(Vec2::ZERO, Vec2::ZERO, 0.0, &bank, true);
        assert_eq!(volley.len(), 3);
    }

    #[test]
    fn port_and_starboard_fire_opposite_sides() {
        let bank = Broadside {
            guns: 1,
            muzzle_speed: 100.0,
            ..Default::default()
        };
        // Facing +X: port is +Y, starboard is -Y.
        let port = broadside_volley(Vec2::ZERO, Vec2::ZERO, 0.0, &bank, true)[0];
        let stbd = broadside_volley(Vec2::ZERO, Vec2::ZERO, 0.0, &bank, false)[0];
        assert!(
            port.velocity.y > 0.0,
            "port should throw +Y, got {}",
            port.velocity.y
        );
        assert!(
            stbd.velocity.y < 0.0,
            "starboard should throw -Y, got {}",
            stbd.velocity.y
        );
    }

    #[test]
    fn balls_inherit_ship_momentum() {
        let bank = Broadside {
            guns: 1,
            ..Default::default()
        };
        let ship_vel = Vec2::new(50.0, 0.0);
        let shot = broadside_volley(Vec2::ZERO, ship_vel, 0.0, &bank, true)[0];
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
