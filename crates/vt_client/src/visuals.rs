//! Turning sim entities into renderable meshes: attaching a mesh+material the
//! first time a ship/projectile/bolt/torpedo appears, keeping torpedoes oriented
//! along their flight, and tinting a ship's material by its live hull/brace/EMP
//! state.

use bevy::prelude::*;
use vt_sim::prelude::*;

use crate::{ship_mesh_label, ship_model, GameMaterials, GameMeshes};

/// Render radius of a cannonball / EMP bolt (the shared sphere is unit-sized).
pub const SHOT_RADIUS: f32 = 7.0;

/// Give every ship without one its faction hull: a low-poly model textured with
/// the faction's colour variant. `base_color` starts white so the painted hull
/// shows through; `damage_tint` then multiplies it for hull/brace/EMP state.
pub fn attach_ship_visuals(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<(Entity, &Faction), (With<Ship>, Without<Mesh3d>)>,
) {
    for (entity, faction) in &ships {
        let (mesh_path, tex_path) = ship_model(faction);
        let mesh: Handle<Mesh> = asset_server.load(ship_mesh_label(mesh_path));
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(asset_server.load(tex_path.to_string())),
            perceptual_roughness: 0.7,
            ..default()
        });
        commands
            .entity(entity)
            .insert((Mesh3d(mesh), MeshMaterial3d(material)));
    }
}

/// Give every cannonball a small glowing sphere (the shared sphere is unit-sized,
/// so scale it up).
pub fn attach_projectile_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    mut shots: Query<(Entity, &mut Transform), (With<Projectile>, Without<Mesh3d>)>,
) {
    for (entity, mut transform) in &mut shots {
        transform.scale = Vec3::splat(SHOT_RADIUS);
        commands.entity(entity).insert((
            Mesh3d(meshes.sphere.clone()),
            MeshMaterial3d(mats.shot.clone()),
        ));
    }
}

/// Give every EMP bolt a small blue sphere.
pub fn attach_empbolt_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    mut bolts: Query<(Entity, &mut Transform), (With<EmpBolt>, Without<Mesh3d>)>,
) {
    for (entity, mut transform) in &mut bolts {
        transform.scale = Vec3::splat(SHOT_RADIUS);
        commands.entity(entity).insert((
            Mesh3d(meshes.sphere.clone()),
            MeshMaterial3d(mats.emp.clone()),
        ));
    }
}

/// Give every torpedo its orange body.
pub fn attach_torpedo_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    torps: Query<Entity, (With<Torpedo>, Without<Mesh3d>)>,
) {
    for entity in &torps {
        commands.entity(entity).insert((
            Mesh3d(meshes.torpedo.clone()),
            MeshMaterial3d(mats.torpedo.clone()),
        ));
    }
}

/// Orient each torpedo body along its 3D velocity so it visibly points up/down
/// at launch and pitches over through the arc.
pub fn orient_torpedoes(mut torps: Query<(&Torpedo, &mut Transform)>) {
    for (torp, mut transform) in &mut torps {
        let dir = torp.vel.normalize_or(Vec3::Z);
        transform.rotation = Quat::from_rotation_arc(Vec3::Z, dir);
    }
}

/// Tint each ship's material — a multiplier over the faction-painted hull:
/// darker as its hull wears down, grey when crippled (boardable), a blue cast
/// while bracing, and a cyan EMP glow as it is disabled by EMP. Faction colour
/// lives in the texture now, so this only carries the ship's *state*.
pub fn damage_tint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<
        (
            &Hull,
            &EmpDefense,
            &MeshMaterial3d<StandardMaterial>,
            Option<&Disabled>,
            Option<&Brace>,
        ),
        With<Ship>,
    >,
) {
    for (hull, emp, material, disabled, brace) in &ships {
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };
        if disabled.is_some() {
            // Crippled hulk — drifting, boardable. Washes the hull toward grey.
            material.base_color = Color::srgb(0.42, 0.44, 0.5);
            continue;
        }
        let frac = (hull.current / hull.max).clamp(0.0, 1.0);
        let k = 0.4 + 0.6 * frac;
        // Neutral multiplier at full hull (white), darkening with damage.
        let (mut r, mut g, mut b) = (k, k, k);
        if brace.is_some_and(|brace| brace.active) {
            // Wash toward a cold brace-blue.
            r *= 0.5;
            g = g * 0.6 + 0.3;
            b = b * 0.5 + 0.5;
        }
        // EMP glow, growing with the EMP load.
        let e = (emp.damage / emp.resist).clamp(0.0, 1.0);
        r = r * (1.0 - e);
        g = g * (1.0 - e) + 0.7 * e;
        b = b * (1.0 - e) + 1.0 * e;
        material.base_color = Color::srgb(r, g, b);
    }
}
