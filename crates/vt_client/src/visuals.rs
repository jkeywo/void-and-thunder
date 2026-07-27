//! Turning sim entities into renderable meshes: attaching a mesh+material the
//! first time a ship/projectile/bolt/torpedo appears, keeping torpedoes oriented
//! along their flight, heeling hulls into their turns, and tinting a ship's
//! material by its live hull/brace/EMP state.

use bevy::prelude::*;
use bevy::time::Fixed;
use vt_sim::prelude::*;

use crate::interpolate::{overstep, SimPose};
use crate::{ship_mesh_label, ship_model, GameMaterials, GameMeshes};

/// Render radius of a cannonball / EMP bolt (the shared sphere is unit-sized).
pub const SHOT_RADIUS: f32 = 7.0;

/// How far a hull heels over at full helm.
const MAX_BANK: f32 = 10.0 * std::f32::consts::PI / 180.0;
/// How quickly the roll catches up to the helm — loose enough that the hull
/// leans into a turn and rights itself afterwards rather than snapping.
const BANK_LERP: f32 = 6.0;
/// How long a hull stays lit up white after taking a hit.
const HIT_FLASH_TIME: f32 = 0.08;

/// A ship's current roll about its own bow axis — the visual heel into a turn.
/// Presentation only: the sim owns [`Heading`] and knows nothing about this.
#[derive(Component, Default)]
pub struct Bank(pub f32);

/// Seconds of white hit-flash left on a hull. Refreshed by every hit that lands
/// on it (see `spawn_hit_effects`), ticked down and folded into the tint here.
#[derive(Component)]
pub struct HitFlash(pub f32);

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
            .insert((Mesh3d(mesh), MeshMaterial3d(material), Bank::default()));
    }
}

/// Heel each hull into its turn, up to [`MAX_BANK`].
///
/// The sim's `movement_system` writes `rotation = from_rotation_z(heading)` in
/// `FixedUpdate`; this runs later in the frame and rewrites the *whole*
/// rotation, so the two never fight over it. The roll is applied about the
/// ship's own bow axis (local +X): port is local +Y, so a negative roll drops
/// the port side, and `Helm::turn` is positive to port — hence the sign, which
/// banks the hull *into* the turn rather than away from it.
pub fn bank_ships(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut ships: Query<(&SimPose, &Helm, &mut Bank, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let k = 1.0 - (-BANK_LERP * dt).exp();
    // Take the heading from the interpolated pose, not the sim's stepped
    // `Heading`: a hull turning at 64 Hz against a higher refresh rate would
    // otherwise snap between facings while its roll eased smoothly.
    let alpha = overstep(&fixed);
    for (pose, helm, mut bank, mut transform) in &mut ships {
        bank.0 += (bank_target(helm.turn) - bank.0) * k;
        transform.rotation =
            Quat::from_rotation_z(pose.heading_at(alpha)) * Quat::from_rotation_x(bank.0);
    }
}

/// The roll a hull settles at for a given helm, in radians about its bow axis.
fn bank_target(turn: f32) -> f32 {
    -turn.clamp(-1.0, 1.0) * MAX_BANK
}

/// Burn down each hull's hit flash. `damage_tint` reads whatever is left.
pub fn tick_hit_flash(
    time: Res<Time>,
    mut commands: Commands,
    mut flashes: Query<(Entity, &mut HitFlash)>,
) {
    let dt = time.delta_secs();
    for (entity, mut flash) in &mut flashes {
        flash.0 -= dt;
        if flash.0 <= 0.0 {
            commands.entity(entity).remove::<HitFlash>();
        }
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

/// Give every burning barrel a sphere the size of the fire it actually is.
///
/// Scaled from the sim's own `radius`, so what the player steers around is the
/// hitbox rather than an artist's guess at it. Deliberately left out of the
/// interpolation filter — a barrel never moves, so smoothing it would be work
/// with nothing to smooth.
pub fn attach_barrel_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    mut barrels: Query<(Entity, &FireBarrel, &mut Transform), Without<Mesh3d>>,
) {
    for (entity, barrel, mut transform) in &mut barrels {
        transform.scale = Vec3::splat(barrel.radius);
        commands.entity(entity).insert((
            Mesh3d(meshes.sphere.clone()),
            MeshMaterial3d(mats.barrel.clone()),
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
///
/// A live [`HitFlash`] is blended in last, over everything else, so a hull lights
/// up white the instant it is struck no matter what state it was already in.
pub fn damage_tint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<
        (
            &Hull,
            &EmpDefense,
            &MeshMaterial3d<StandardMaterial>,
            Option<&Disabled>,
            Option<&Brace>,
            Option<&HitFlash>,
        ),
        With<Ship>,
    >,
) {
    for (hull, emp, material, disabled, brace, flash) in &ships {
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };
        // How far toward white this hull is flashing right now.
        let flash = flash.map_or(0.0, |f| (f.0 / HIT_FLASH_TIME).clamp(0.0, 1.0));
        if disabled.is_some() {
            // Crippled hulk — drifting, boardable. Washes the hull toward grey.
            material.base_color = flashed(Color::srgb(0.42, 0.44, 0.5), flash);
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
        r *= 1.0 - e;
        g = g * (1.0 - e) + 0.7 * e;
        b = b * (1.0 - e) + 1.0 * e;
        material.base_color = flashed(Color::srgb(r, g, b), flash);
    }
}

/// Blend a tint toward white by `flash` (0 = untouched, 1 = fully lit).
fn flashed(color: Color, flash: f32) -> Color {
    if flash <= 0.0 {
        return color;
    }
    let c = color.to_srgba();
    Color::srgb(
        c.red + (1.0 - c.red) * flash,
        c.green + (1.0 - c.green) * flash,
        c.blue + (1.0 - c.blue) * flash,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hull must heel *into* its turn. Port helm is `turn > 0` and port is the
    /// ship's local +Y, so the roll about the bow axis has to be negative to
    /// drop that side — get this backwards and every ship leans out of its
    /// turns like a motorbike ridden wrong.
    #[test]
    fn a_hull_heels_into_its_turn() {
        assert!(
            bank_target(1.0) < 0.0,
            "a turn to port should drop the port side"
        );
        assert!(
            bank_target(-1.0) > 0.0,
            "a turn to starboard should drop the starboard side"
        );
        assert_eq!(bank_target(0.0), 0.0, "a centred helm should sit level");
    }

    #[test]
    fn the_heel_is_capped_at_ten_degrees() {
        for turn in [1.0, 5.0, -5.0, f32::MAX] {
            let degrees = bank_target(turn).to_degrees().abs();
            assert!(
                degrees <= 10.0 + 1e-3,
                "helm {turn} rolled {degrees}°, past the 10° cap"
            );
        }
        assert!((bank_target(1.0).to_degrees().abs() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_full_flash_lights_a_hull_white() {
        let lit = flashed(Color::srgb(0.2, 0.1, 0.05), 1.0).to_srgba();
        assert!(lit.red > 0.99 && lit.green > 0.99 && lit.blue > 0.99);
    }

    #[test]
    fn no_flash_leaves_the_tint_alone() {
        let base = Color::srgb(0.2, 0.1, 0.05);
        let kept = flashed(base, 0.0).to_srgba();
        assert!((kept.red - 0.2).abs() < 1e-6 && (kept.blue - 0.05).abs() < 1e-6);
    }
}
