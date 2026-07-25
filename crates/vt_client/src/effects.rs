//! Short-lived "juice" effects: muzzle flashes, hit sparks, EMP crackles and
//! destruction blasts, each a scaling/fading sphere that despawns at the end of
//! its life. Hits and kills also add camera-shake trauma.

use bevy::prelude::*;
use vt_sim::prelude::*;

use crate::camera::CameraRig;
use crate::GameMeshes;

/// A short-lived visual effect (muzzle flash, hit spark, explosion) that scales
/// and fades out over its life, then despawns.
#[derive(Component)]
pub struct Effect {
    age: f32,
    life: f32,
    start_scale: f32,
    end_scale: f32,
    color: Color,
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
        },
    ));
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

/// Sparks and a little shake when a hull is hit.
pub fn spawn_hit_effects(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut hits: MessageReader<ShipHit>,
    mut rig: ResMut<CameraRig>,
) {
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
        rig.add_trauma(0.12);
    }
}

/// An expanding blast and a bigger shake when a ship is destroyed.
pub fn spawn_destroy_effects(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut destroyed: MessageReader<ShipDestroyed>,
    mut rig: ResMut<CameraRig>,
) {
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
        rig.add_trauma(0.45);
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
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.base_color = effect.color.with_alpha(1.0 - t);
        }
    }
}
