//! The central star: an animated churning surface plus a billboarded corona.
//!
//! Ported from `project-phoenix-v2`'s `entities/star.rs`. Two custom materials
//! drive `assets/shaders/star_surface.wgsl` and `star_halo.wgsl`; their `time`
//! uniform is advanced every frame so the surface flows and the halo pulses.
//! Phoenix's per-world `StarConfig` is replaced here with fixed constants tuned
//! to the game's warm sun.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, PrimitiveTopology},
    shader::ShaderRef,
};
use vt_sim::prelude::Landmark;

const STAR_SURFACE_SHADER: &str = "shaders/star_surface.wgsl";
const STAR_HALO_SHADER: &str = "shaders/star_halo.wgsl";

// Warm-sun palette (linear RGB). Surface is the base, hot is the flare colour
// the churn pushes toward, cell is the cooler granulation between cells.
const SURFACE_COLOUR: [f32; 3] = [1.0, 0.72, 0.30];
const HOT_COLOUR: [f32; 3] = [1.0, 0.95, 0.72];
const CELL_COLOUR: [f32; 3] = [0.85, 0.35, 0.12];
const HALO_COLOUR: [f32; 3] = [1.0, 0.70, 0.35];
const ANIMATION_SPEED: f32 = 1.0;
/// The corona quad's radius as a multiple of the star's surface radius.
const HALO_SCALE: f32 = 2.4;

/// Marker for the star's corona quad, so it can be billboarded at the camera.
#[derive(Component, Clone, Copy, Debug)]
pub struct StarHalo;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct StarSurfaceMaterial {
    #[uniform(0)]
    pub surface_r: f32,
    #[uniform(0)]
    pub surface_g: f32,
    #[uniform(0)]
    pub surface_b: f32,
    #[uniform(0)]
    pub _pad0: f32,
    #[uniform(0)]
    pub hot_r: f32,
    #[uniform(0)]
    pub hot_g: f32,
    #[uniform(0)]
    pub hot_b: f32,
    #[uniform(0)]
    pub time: f32,
    #[uniform(0)]
    pub cell_r: f32,
    #[uniform(0)]
    pub cell_g: f32,
    #[uniform(0)]
    pub cell_b: f32,
    #[uniform(0)]
    pub animation_speed: f32,
}

impl Material for StarSurfaceMaterial {
    fn fragment_shader() -> ShaderRef {
        STAR_SURFACE_SHADER.into()
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct StarHaloMaterial {
    #[uniform(0)]
    pub color_r: f32,
    #[uniform(0)]
    pub color_g: f32,
    #[uniform(0)]
    pub color_b: f32,
    #[uniform(0)]
    pub alpha: f32,
    #[uniform(0)]
    pub time: f32,
    #[uniform(0)]
    pub animation_speed: f32,
    #[uniform(0)]
    pub _pad0: f32,
    #[uniform(0)]
    pub _pad1: f32,
}

impl Material for StarHaloMaterial {
    fn fragment_shader() -> ShaderRef {
        STAR_HALO_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Registers both star materials and the systems that animate + billboard them.
pub struct StarPlugin;

impl Plugin for StarPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<StarSurfaceMaterial>::default())
            .add_plugins(MaterialPlugin::<StarHaloMaterial>::default())
            .add_systems(Update, (tick_star_materials, billboard_star_halos));
    }
}

fn surface_material() -> StarSurfaceMaterial {
    StarSurfaceMaterial {
        surface_r: SURFACE_COLOUR[0],
        surface_g: SURFACE_COLOUR[1],
        surface_b: SURFACE_COLOUR[2],
        _pad0: 0.0,
        hot_r: HOT_COLOUR[0],
        hot_g: HOT_COLOUR[1],
        hot_b: HOT_COLOUR[2],
        time: 0.0,
        cell_r: CELL_COLOUR[0],
        cell_g: CELL_COLOUR[1],
        cell_b: CELL_COLOUR[2],
        animation_speed: ANIMATION_SPEED,
    }
}

fn halo_material() -> StarHaloMaterial {
    StarHaloMaterial {
        color_r: HALO_COLOUR[0],
        color_g: HALO_COLOUR[1],
        color_b: HALO_COLOUR[2],
        alpha: 0.55,
        time: 0.0,
        animation_speed: ANIMATION_SPEED,
        _pad0: 0.0,
        _pad1: 0.0,
    }
}

/// Spawn the star at `pos` on the play plane: an animated surface sphere plus a
/// camera-facing corona quad. Returns nothing — the pieces are independent
/// top-level entities so the halo billboards cleanly against the camera.
pub fn spawn_star(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    surface_materials: &mut Assets<StarSurfaceMaterial>,
    halo_materials: &mut Assets<StarHaloMaterial>,
    pos: Vec2,
    radius: f32,
) {
    let surface_mesh = meshes.add(uv_sphere_mesh(radius, 48, 32));
    commands.spawn((
        Mesh3d(surface_mesh),
        MeshMaterial3d(surface_materials.add(surface_material())),
        Transform::from_translation(pos.extend(0.0)),
        // The star is a body like any other. Without this it carried no
        // Landmark at all, so it had no collision presence whatsoever and ships
        // and shot flew straight through the middle of it. The radius is the
        // *surface* radius, not the corona's — the halo is light, and light is
        // not something you bump into.
        Landmark { radius },
    ));

    let halo_radius = radius * HALO_SCALE;
    let halo_mesh = meshes.add(halo_quad_mesh(halo_radius));
    commands.spawn((
        Mesh3d(halo_mesh),
        MeshMaterial3d(halo_materials.add(halo_material())),
        Transform::from_translation(pos.extend(0.0)),
        StarHalo,
    ));
}

fn uv_sphere_mesh(radius: f32, longitude_segments: u32, latitude_segments: u32) -> Mesh {
    let radius = radius.max(0.1);
    let longitudes = longitude_segments.max(8);
    let latitudes = latitude_segments.max(4);

    let mut positions = Vec::with_capacity(((longitudes + 1) * (latitudes + 1)) as usize);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut uvs = Vec::with_capacity(positions.capacity());

    for lat in 0..=latitudes {
        let v = lat as f32 / latitudes as f32;
        let theta = v * std::f32::consts::PI;
        let sin_theta = theta.sin();
        let cos_theta = theta.cos();

        for lon in 0..=longitudes {
            let u = lon as f32 / longitudes as f32;
            let phi = u * std::f32::consts::TAU;
            let normal = Vec3::new(phi.cos() * sin_theta, cos_theta, phi.sin() * sin_theta);

            positions.push((normal * radius).to_array());
            normals.push(normal.to_array());
            uvs.push([u, v]);
        }
    }

    let stride = longitudes + 1;
    let mut indices = Vec::with_capacity((longitudes * latitudes * 6) as usize);
    for lat in 0..latitudes {
        for lon in 0..longitudes {
            let a = lat * stride + lon;
            let b = a + stride;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn halo_quad_mesh(radius: f32) -> Mesh {
    let r = radius.max(0.1);
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-r, -r, 0.0], [r, -r, 0.0], [r, r, 0.0], [-r, r, 0.0]],
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        vec![
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    )
    .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

fn tick_star_materials(
    time: Res<Time>,
    mut surface_materials: ResMut<Assets<StarSurfaceMaterial>>,
    mut halo_materials: ResMut<Assets<StarHaloMaterial>>,
) {
    let elapsed = time.elapsed_secs();
    for (_, material) in surface_materials.iter_mut() {
        material.time = elapsed;
    }
    for (_, material) in halo_materials.iter_mut() {
        material.time = elapsed;
    }
}

fn billboard_star_halos(
    camera: Query<&GlobalTransform, (With<Camera3d>, Without<StarHalo>)>,
    mut halos: Query<&mut Transform, With<StarHalo>>,
) {
    let Some(camera_transform) = camera.iter().next() else {
        return;
    };
    // Screen-align the corona: give the quad (local +Z normal, +Y up) the
    // camera's own rotation so it's parallel to the view plane and stays
    // concentric with the star. `look_to` toward the star's position went
    // degenerate in V&T's +Z-up world (Phoenix was +Y-up) and pushed the large
    // quad off-centre — a second disc beside the sun.
    let cam_rotation = camera_transform.rotation();
    for mut transform in &mut halos {
        transform.rotation = cam_rotation;
    }
}
