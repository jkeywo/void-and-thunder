//! Short-lived "juice" effects: muzzle flashes, hit sparks, EMP crackles and
//! destruction blasts, each a scaling/fading sphere that despawns at the end of
//! its life.
//!
//! This is also where impact *feel* is routed. The sim's [`ShipHit`] and
//! [`ShipDestroyed`] name the hull involved, so a hit on the player's own ship
//! shakes and shoves the camera hard while a distant one barely registers, and
//! every hit lights its hull up white for a frame or two.

use bevy::prelude::*;
use vt_sim::prelude::*;

use crate::bullet_time::Hitstop;
use crate::camera::CameraRig;
use crate::visuals::HitFlash;
use crate::{GameMeshes, Player};

/// Beyond this range a hit is too far away to be felt at all.
const SHAKE_RANGE: f32 = 900.0;
/// Trauma from a hit on the player's own hull: a floor, plus a share of the
/// blow's weight so a torpedo lands harder than a cannonball.
const OWN_HIT_TRAUMA: f32 = 0.18;
const OWN_HIT_TRAUMA_PER_DAMAGE: f32 = 0.010;
/// How hard an impact shoves the camera, in world units.
const OWN_HIT_KICK: f32 = 22.0;
/// Trauma from someone else's hull being hit, at point-blank range.
const NEARBY_HIT_TRAUMA: f32 = 0.10;
/// Trauma from a kill: your own death rocks the view, others less so.
const OWN_DEATH_TRAUMA: f32 = 0.9;
const NEARBY_DEATH_TRAUMA: f32 = 0.45;
/// Freeze-frame lengths, in real seconds.
const KILL_HITSTOP: f32 = 0.06;
const OWN_HIT_HITSTOP: f32 = 0.04;
/// Shrapnel thrown by a destroyed hull.
const DEBRIS_COUNT: u32 = 8;
const DEBRIS_SPEED: (f32, f32) = (90.0, 220.0);
const DEBRIS_LIFE: f32 = 0.35;
/// Seconds a struck hull stays lit up white.
const FLASH_TIME: f32 = 0.08;

/// A short-lived visual effect (muzzle flash, hit spark, explosion) that scales
/// and fades out over its life, then despawns.
#[derive(Component)]
pub struct Effect {
    age: f32,
    life: f32,
    start_scale: f32,
    end_scale: f32,
    color: Color,
    /// Drift while it lives — zero for a blast that blooms in place, non-zero
    /// for debris thrown clear of a wreck.
    vel: Vec2,
}

/// Spawn a scaling, fading effect sphere at a world position.
fn spawn_effect(
    commands: &mut Commands,
    meshes: &GameMeshes,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec2,
    start_scale: f32,
    end_scale: f32,
    life: f32,
    color: Color,
) {
    spawn_moving_effect(
        commands,
        meshes,
        materials,
        pos,
        Vec2::ZERO,
        start_scale,
        end_scale,
        life,
        color,
    );
}

/// As [`spawn_effect`], but the sphere drifts at `vel` while it fades.
#[allow(clippy::too_many_arguments)]
fn spawn_moving_effect(
    commands: &mut Commands,
    meshes: &GameMeshes,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec2,
    vel: Vec2,
    start_scale: f32,
    end_scale: f32,
    life: f32,
    color: Color,
) {
    let material = materials.add(StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.sphere.clone()),
        MeshMaterial3d(material),
        Transform::from_translation(pos.extend(4.0)).with_scale(Vec3::splat(start_scale)),
        Effect {
            age: 0.0,
            life,
            start_scale,
            end_scale,
            color,
            vel,
        },
    ));
}

/// How much undirected shake an event at `pos` deserves from the camera's
/// current focus — full strength underfoot, nothing past [`SHAKE_RANGE`].
fn falloff(pos: Vec2, focus: Vec2) -> f32 {
    (1.0 - pos.distance(focus) / SHAKE_RANGE).clamp(0.0, 1.0)
}

/// A muzzle flash blooms wherever a new cannonball appears.
pub fn muzzle_flashes(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    new_shots: Query<&Transform, Added<Projectile>>,
) {
    for transform in &new_shots {
        spawn_effect(
            &mut commands,
            &meshes,
            &mut materials,
            transform.translation.truncate(),
            13.0,
            2.0,
            0.12,
            Color::srgb(1.0, 0.95, 0.6),
        );
    }
}

/// A blue crackle where an EMP bolt lands.
pub fn spawn_emp_effects(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut impacts: MessageReader<EmpImpact>,
) {
    for impact in impacts.read() {
        spawn_effect(
            &mut commands,
            &meshes,
            &mut materials,
            impact.position,
            5.0,
            22.0,
            0.28,
            Color::srgb(0.5, 0.8, 1.0),
        );
    }
}

/// Sparks where a hull is hit, a white flash on the hull itself, and a shake
/// scaled to whether it was *your* hull.
///
/// Taking a hit is the one moment the camera should stop being a neutral
/// observer: your own damage shakes hard, shoves the view along the blow's
/// travel and stutters time; everyone else's is a distant rumble that falls off
/// with range.
pub fn spawn_hit_effects(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut hits: MessageReader<ShipHit>,
    mut rig: ResMut<CameraRig>,
    mut hitstop: ResMut<Hitstop>,
    player: Query<(Entity, &Transform), With<Player>>,
) {
    let player = player.single().ok();
    for hit in hits.read() {
        spawn_effect(
            &mut commands,
            &meshes,
            &mut materials,
            hit.position,
            6.0,
            18.0,
            0.22,
            Color::srgb(1.0, 0.7, 0.3),
        );
        // Light the struck hull up. It may already have been despawned this
        // frame (destruction resolves in the same sim step), so insert only if
        // it is still there.
        if let Ok(mut ship) = commands.get_entity(hit.ship) {
            ship.insert(HitFlash(FLASH_TIME));
        }

        match player {
            Some((entity, transform)) if entity == hit.ship => {
                rig.add_trauma(OWN_HIT_TRAUMA + hit.damage * OWN_HIT_TRAUMA_PER_DAMAGE);
                // Shove the view the way the shot was travelling: outward from
                // the hull's centre through the point of impact.
                let along = hit.position - transform.translation.truncate();
                rig.add_kick(along, OWN_HIT_KICK);
                hitstop.freeze(OWN_HIT_HITSTOP);
            }
            _ => {
                let felt = falloff(hit.position, rig.focus());
                rig.add_trauma(NEARBY_HIT_TRAUMA * felt);
            }
        }
    }
}

/// An expanding blast, a spray of shrapnel, a freeze-frame and a big shake when
/// a ship is destroyed.
pub fn spawn_destroy_effects(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut destroyed: MessageReader<ShipDestroyed>,
    mut rig: ResMut<CameraRig>,
    mut hitstop: ResMut<Hitstop>,
    mut seed: Local<u32>,
    player: Query<Entity, With<Player>>,
) {
    // The player entity is despawned in the same step as its own death message,
    // so this query is empty exactly when the kill *was* the player.
    let player = player.single().ok();
    for kill in destroyed.read() {
        spawn_effect(
            &mut commands,
            &meshes,
            &mut materials,
            kill.position,
            14.0,
            85.0,
            0.5,
            Color::srgb(1.0, 0.6, 0.25),
        );

        // Shrapnel: a ring of sparks thrown clear of the wreck, each on its own
        // bearing and at its own speed so the spray never looks stamped out.
        for i in 0..DEBRIS_COUNT {
            let spread = lcg_next(&mut seed) - 0.5;
            let angle = (i as f32 / DEBRIS_COUNT as f32 + spread * 0.15) * std::f32::consts::TAU;
            let speed = DEBRIS_SPEED.0 + lcg_next(&mut seed) * (DEBRIS_SPEED.1 - DEBRIS_SPEED.0);
            spawn_moving_effect(
                &mut commands,
                &meshes,
                &mut materials,
                kill.position,
                Vec2::from_angle(angle) * speed,
                5.0,
                1.0,
                DEBRIS_LIFE,
                Color::srgb(1.0, 0.8, 0.45),
            );
        }

        hitstop.freeze(KILL_HITSTOP);
        let own_death = player.is_none_or(|entity| entity == kill.ship);
        if own_death {
            rig.add_trauma(OWN_DEATH_TRAUMA);
        } else {
            let felt = falloff(kill.position, rig.focus());
            rig.add_trauma(NEARBY_DEATH_TRAUMA * felt);
        }
    }
}

/// Advance every effect: scale over its life and fade to nothing, then despawn.
pub fn update_effects(
    time: Res<Time>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut effects: Query<(
        Entity,
        &mut Effect,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, mut effect, mut transform, material) in &mut effects {
        effect.age += dt;
        let t = (effect.age / effect.life).clamp(0.0, 1.0);
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let scale = effect.start_scale + (effect.end_scale - effect.start_scale) * t;
        transform.scale = Vec3::splat(scale);
        transform.translation += (effect.vel * dt).extend(0.0);
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.base_color = effect.color.with_alpha(1.0 - t);
        }
    }
}
