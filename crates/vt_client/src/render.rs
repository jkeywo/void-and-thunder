//! Space skybox: the starfield cubemap that wraps the scene.
//!
//! Ported from `project-phoenix-v2`'s `render_setup.rs`. The image ships as a
//! vertical 6-face strip (a PNG), which wgpu will not sample as a cubemap, so
//! [`prepare_space_skybox_cubemap`] reinterprets it as a 6-layer cube array
//! exactly once, after it finishes loading.

use bevy::core_pipeline::Skybox;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureViewDescriptor, TextureViewDimension};

/// Vertical 6-face cubemap strip, reinterpreted as a cube array at runtime.
pub const SPACE_SKYBOX_PATH: &str = "skybox/phoenix_space_cubemap.png";
/// How brightly the skybox lights the scene as an environment map.
pub const SPACE_SKYBOX_BRIGHTNESS: f32 = 450.0;

/// The skybox image handle plus a latch so the cubemap reinterpretation runs
/// exactly once, after the PNG finishes loading.
#[derive(Resource)]
pub struct SpaceSkyboxAsset {
    pub image: Handle<Image>,
    pub is_loaded: bool,
}

impl FromWorld for SpaceSkyboxAsset {
    fn from_world(world: &mut World) -> Self {
        let image = world.resource::<AssetServer>().load(SPACE_SKYBOX_PATH);
        Self {
            image,
            is_loaded: false,
        }
    }
}

/// Owns the space skybox asset and its one-shot cubemap conversion. Add this
/// before spawning any camera carrying [`space_skybox`].
pub struct SpaceSkyboxPlugin;

impl Plugin for SpaceSkyboxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpaceSkyboxAsset>()
            .add_systems(Update, prepare_space_skybox_cubemap);
    }
}

/// The `Skybox` component for a 3D camera, at the game's brightness.
pub fn space_skybox(skybox: &SpaceSkyboxAsset) -> Skybox {
    Skybox {
        image: Some(skybox.image.clone()),
        brightness: SPACE_SKYBOX_BRIGHTNESS,
        ..default()
    }
}

/// One-shot: the skybox PNG ships as a vertical 6-face strip, which wgpu will
/// not sample as a cubemap. Once loaded, reinterpret it as a 6-layer array with
/// a `Cube` texture view, then push the handle onto every skybox camera.
pub fn prepare_space_skybox_cubemap(
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    skybox_asset: Option<ResMut<SpaceSkyboxAsset>>,
    mut skyboxes: Query<&mut Skybox>,
) {
    let Some(mut skybox_asset) = skybox_asset else {
        return;
    };
    if skybox_asset.is_loaded || !asset_server.load_state(&skybox_asset.image).is_loaded() {
        return;
    }

    let Some(mut image) = images.get_mut(&skybox_asset.image) else {
        return;
    };
    if image.texture_descriptor.array_layer_count() == 1 {
        let layers = image.height() / image.width();
        if layers != 6 {
            bevy::log::error!(
                "space skybox expected a vertical 6-face cubemap, got {}x{}",
                image.width(),
                image.height()
            );
            skybox_asset.is_loaded = true;
            return;
        }
        if let Err(err) = image.reinterpret_stacked_2d_as_array(layers) {
            bevy::log::error!("space skybox cubemap conversion failed: {err}");
            skybox_asset.is_loaded = true;
            return;
        }

        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        });
    }

    for mut skybox in &mut skyboxes {
        skybox.image = Some(skybox_asset.image.clone());
    }
    skybox_asset.is_loaded = true;
}
