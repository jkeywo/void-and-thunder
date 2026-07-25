//! # Void & Thunder — Client
//!
//! The Bevy front end: window, renderer, camera and input. It owns *no* game
//! rules — those live in [`vt_sim`]. This crate spawns ships (sim entities),
//! translates input into the sim's [`Helm`]/[`FireOrders`] intent components,
//! draws sim entities as 3D meshes, and mounts [`SimPlugin`].
//!
//! **2.5D presentation.** The simulation is flat: it works in the XY plane with
//! heading as a rotation about Z. The client treats **+Z as up** and puts a
//! perspective camera slightly above that plane, looking gently down and yawing
//! around the player's ship. So the gameplay stays a Black-Flag naval duel while
//! the view is fully 3D — boxes for hulls, spheres for shot and worlds.
//!
//! Controls — aim with the mouse (or right stick). Aiming a broadside or a
//! microwarp dilates time (bullet-time) from a rechargeable aim battery.
//! Torpedo locking does not: its locks accrue on their own timer, so the sweep
//! is unhurried already.
//!   W / S      — throttle forward / reverse
//!   A / D      — turn to port / starboard
//!   LMB / RMB  — hold to aim the port / starboard broadside; the horizontal aim
//!                axis sweeps the volley across the arc and the camera yaw with
//!                it. Release to fire.
//!   Q          — EMP: hold to auto-track and drain a target's drive
//!   Left Ctrl  — torpedoes: hold to lock (1 + 1 per 0.5s), release to volley.
//!                The camera lifts directly overhead for a top-down aim.
//!   Left Shift — microwarp: hold to place a teleport point (top-down), release
//!                to warp
//!   Space      — boost (rechargeable battery)
//!   C          — brace (cut incoming damage)
//!   Board      — hold position within range of a crippled hulk for 3s to loot it
//!                (a ring fills to show progress; no key needed)
//!   T          — toggle AI pilot (the AI flies the ship; you keep the camera)
//!   P / Esc    — pause / resume
//!   R          — restart after a run ends
//!   F11        — toggle borderless fullscreen
//!   F1         — design panel (dev builds only; pauses while it is open)
//!
//! Broadside banks reload independently (port/starboard) and their aim beams glow
//! amber when loaded, dim red while reloading. Reverse is ~25% of forward speed.
//!
//! Gamepad: left stick throttle/steer, right stick aim/camera (rate-based, like a
//! mouse, outside broadside aiming), LT/RT broadsides, LB torpedoes, RB microwarp,
//! X EMP, A boost, Y brace, Start pause (restart on the game-over screen).

use bevy::asset::AssetPath;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use vt_sim::prelude::*;

mod data;
use data::DataPlugin;

mod dev;
use dev::{game_has_input, game_has_pointer, DevPanelPlugin};

mod audio;
use audio::SfxPlugin;

mod render;
use render::{space_skybox, SpaceSkyboxAsset, SpaceSkyboxPlugin};

mod star;
use star::{spawn_star, StarHaloMaterial, StarPlugin, StarSurfaceMaterial};

mod hud;
use hud::HudBridgePlugin;

mod camera;
use camera::{camera_orbit, CameraRig, FreeLook, MainCamera};

mod visuals;
use visuals::{
    attach_empbolt_visuals, attach_projectile_visuals, attach_ship_visuals, attach_torpedo_visuals,
    bank_ships, damage_tint, orient_torpedoes, tick_hit_flash,
};

mod input;
use input::{
    player_input, toggle_controls_panel, toggle_fullscreen, toggle_pause, toggle_player_ai,
    track_input_method, AimCursor, Aiming, BroadsideAim, ControlsPanel, InputMethod, Paused,
    PlayerAi,
};

mod gizmos;
use gizmos::{
    draw_aim_beams, draw_aim_lead, draw_boarding, draw_charge_telegraph, draw_grid,
    draw_microwarp_range, draw_reticle, draw_torpedo_locks, draw_torpedo_range, microwarp_ghost,
    MicrowarpGhost,
};

mod effects;
use effects::{
    muzzle_flashes, spawn_destroy_effects, spawn_emp_effects, spawn_hit_effects, update_effects,
};

mod edge_markers;
use edge_markers::{update_offscreen_markers, EdgeMarker, EDGE_MARKER_COUNT, EDGE_MARKER_SIZE};

mod interpolate;
use interpolate::{
    attach_sim_pose, interpolate_sim_pose, record_sim_pose, restore_sim_pose, SmoothingSet,
};

mod trails;
use trails::TrailPlugin;

mod bullet_time;
use bullet_time::{aim_time_dilation, AimBattery, Hitstop};

mod session;
use session::{
    await_data, clear_field, freeze_for_menu, restart, start_run, unfreeze_for_run, watch_outcome,
    GameState,
};

// ---- Presentation constants ----

// Placeholder ship hulls: CC0 low-poly models from Quaternius's Ultimate
// Spaceships pack (see assets/CREDITS.md), converted to glb oriented bow-along-+X
// and normalised to ~44-unit length. Each faction gets a distinct hull plus the
// pack's matching colour variant, so faction identity comes from the textured
// paint while `damage_tint` only multiplies in the hull/brace/EMP state.
const SHIP_MODEL_PLAYER: &str = "models/challenger.glb";

/// The glb + colour-variant texture placeholder for a faction's hull.
///
/// The Corsairs fly the pack's chunkiest hull, because it is the one the player
/// looks at all game. Measured across the pack's bounding boxes, the Challenger
/// is nearly as wide as it is long and half again as tall as the Executioner it
/// replaced — a corsair sloop with some heft rather than a flat dart. It and the
/// Executioner simply swapped factions; every hull stays unique to its power,
/// and each keeps its own baked texture (there is one colour variant per model
/// in the repo, so hull and paint move together).
fn ship_model(faction: &Faction) -> (&'static str, &'static str) {
    match faction {
        Faction::Corsairs => (
            "models/challenger.glb",
            "models/textures/challenger_purple.png",
        ),
        Faction::Houses => ("models/imperial.glb", "models/textures/imperial_red.png"),
        Faction::Janissariat => ("models/bob.glb", "models/textures/bob_orange.png"),
        Faction::Guild => (
            "models/dispatcher.glb",
            "models/textures/dispatcher_blue.png",
        ),
        Faction::Freebooters => (
            "models/executioner.glb",
            "models/textures/executioner_green.png",
        ),
    }
}

/// The asset path for a ship glb's single mesh primitive (`glb#Mesh0/Primitive0`),
/// so the label convention lives in one place.
fn ship_mesh_label(path: &str) -> AssetPath<'static> {
    GltfAssetLabel::Primitive {
        mesh: 0,
        primitive: 0,
    }
    .from_asset(path.to_string())
}

/// Marker for the entity the local player controls.
#[derive(Component)]
pub struct Player;

/// Shared meshes, built once at startup.
#[derive(Resource)]
pub struct GameMeshes {
    ship: Handle<Mesh>,
    /// Unit sphere — scaled per use.
    pub sphere: Handle<Mesh>,
    /// A long thin torpedo body (oriented along its velocity).
    pub torpedo: Handle<Mesh>,
}

/// Shared materials for things that never change colour.
#[derive(Resource)]
pub struct GameMaterials {
    pub shot: Handle<StandardMaterial>,
    pub emp: Handle<StandardMaterial>,
    pub torpedo: Handle<StandardMaterial>,
    ghost: Handle<StandardMaterial>,
}

fn main() {
    let default_plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "Void & Thunder".into(),
                // Let the canvas fill its parent element on the web.
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        })
        // We ship no `.meta` sidecars. Skip the meta lookup entirely so assets
        // load cleanly on the web, where a dev server (trunk) answers the missing
        // `.meta` with a 200 + index.html that Bevy then fails to parse as RON —
        // which otherwise breaks every model/texture load.
        .set(AssetPlugin {
            meta_check: bevy::asset::AssetMetaCheck::Never,
            ..default()
        })
        .set(ImagePlugin::default_nearest());
    // On the web, Bevy audio is disabled — sound goes through a WebAudio shim
    // (see src/audio.rs). Native keeps Bevy audio.
    #[cfg(target_arch = "wasm32")]
    let default_plugins = default_plugins.disable::<bevy::audio::AudioPlugin>();

    App::new()
        .add_plugins(default_plugins)
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .add_plugins(SimPlugin)
        // After SimPlugin: it installs the tuning resources this loads over.
        .add_plugins(DataPlugin)
        // A no-op without the `dev-panel` feature, but always mounted so the
        // input guards below can be unconditional.
        .add_plugins(DevPanelPlugin)
        .add_plugins(SfxPlugin)
        .add_plugins(SpaceSkyboxPlugin)
        .add_plugins(StarPlugin)
        .add_plugins(HudBridgePlugin)
        .add_plugins(TrailPlugin)
        .init_state::<GameState>()
        .init_resource::<CameraRig>()
        .init_resource::<Aiming>()
        .init_resource::<AimBattery>()
        .init_resource::<InputMethod>()
        .init_resource::<PlayerAi>()
        .init_resource::<Paused>()
        .init_resource::<FreeLook>()
        .init_resource::<AimCursor>()
        .init_resource::<BroadsideAim>()
        .init_resource::<ControlsPanel>()
        .init_resource::<Hitstop>()
        .add_systems(Startup, setup)
        // Loading lays out the encounter from data. It clears the field on the
        // way in, so re-entering it (picking the test range) swaps scenarios
        // rather than stacking one on top of the other.
        .add_systems(OnEnter(GameState::Loading), (clear_field, freeze_for_menu))
        .add_systems(Update, await_data.run_if(in_state(GameState::Loading)))
        // The start screen freezes the sim; casting off thaws it.
        .add_systems(OnEnter(GameState::Menu), freeze_for_menu)
        .add_systems(OnEnter(GameState::Playing), unfreeze_for_run)
        // Smoothing: the sim steps at a fixed 64 Hz, the screen redraws far more
        // often. Hand the sim back its authoritative pose before each step, take
        // the new one after, and draw the blend — otherwise the whole world
        // visibly stutters against a camera that eases on real time.
        .add_systems(FixedFirst, restore_sim_pose)
        .add_systems(FixedLast, record_sim_pose)
        .add_systems(
            Update,
            (attach_sim_pose, interpolate_sim_pose)
                .chain()
                .in_set(SmoothingSet),
        )
        // Presentation runs in every state, and after the smoothing above so it
        // reads the pose actually on screen. Split in two: giving sim entities
        // their bodies, then the overlays drawn on top of them. (Bevy's system
        // tuples top out at 20 elements, so these cannot be one list anyway.)
        .add_systems(
            Update,
            (
                attach_ship_visuals,
                attach_projectile_visuals,
                attach_empbolt_visuals,
                attach_torpedo_visuals,
                orient_torpedoes,
                // `bank_ships` owns each hull's rotation, so it must land after
                // the sim's movement (FixedUpdate) has written the flat heading.
                bank_ships,
                tick_hit_flash,
                damage_tint.after(tick_hit_flash),
                microwarp_ghost,
                camera_orbit.run_if(game_has_pointer),
                aim_time_dilation,
            )
                .after(SmoothingSet),
        )
        // Gizmo overlays: aim beams and leads, telegraphs, ranges, markers.
        .add_systems(
            Update,
            (
                draw_grid,
                draw_aim_beams,
                draw_aim_lead,
                draw_charge_telegraph,
                draw_reticle,
                draw_torpedo_locks,
                draw_torpedo_range,
                draw_boarding,
                draw_microwarp_range,
                update_offscreen_markers,
            )
                .after(SmoothingSet),
        )
        // Juice: muzzle flashes, hit sparks, explosions, screen shake.
        .add_systems(
            Update,
            (
                muzzle_flashes,
                spawn_hit_effects,
                spawn_destroy_effects,
                spawn_emp_effects,
                update_effects,
            ),
        )
        // Playing: take input and watch for win/lose.
        // Every input system yields to the design panel: without this, dragging
        // a slider across the viewport also sweeps the broadside arc and fires
        // on release, because these read the raw mouse and keyboard.
        .add_systems(
            Update,
            (player_input, toggle_player_ai, toggle_pause)
                .run_if(in_state(GameState::Playing))
                .run_if(game_has_input),
        )
        .add_systems(Update, watch_outcome.run_if(in_state(GameState::Playing)))
        .add_systems(
            Update,
            (track_input_method, toggle_controls_panel).run_if(game_has_input),
        )
        // Window management answers whatever else has focus, so it is not gated
        // on the design panel's input guard.
        .add_systems(Update, toggle_fullscreen)
        // Start screen: wait for the player to cast off.
        .add_systems(
            Update,
            start_run
                .run_if(in_state(GameState::Menu))
                .run_if(game_has_input),
        )
        // Game over: wait for a restart.
        .add_systems(
            Update,
            restart
                .run_if(in_state(GameState::GameOver))
                .run_if(game_has_input),
        )
        .run();
}

fn setup(
    mut commands: Commands,
    bounds: Res<SystemBounds>,
    skybox: Res<SpaceSkyboxAsset>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut star_surface_materials: ResMut<Assets<StarSurfaceMaterial>>,
    mut star_halo_materials: ResMut<Assets<StarHaloMaterial>>,
) {
    // Shared meshes and materials.
    let game_meshes = GameMeshes {
        // The microwarp ghost borrows the player's hull (the corsair Executioner).
        ship: asset_server.load(ship_mesh_label(SHIP_MODEL_PLAYER)),
        sphere: meshes.add(Sphere::new(1.0)),
        // Long in local +Z, so orienting +Z along velocity points it forward.
        torpedo: meshes.add(Cuboid::new(5.0, 5.0, 26.0)),
    };
    let game_materials = GameMaterials {
        shot: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.9, 0.4),
            unlit: true,
            ..default()
        }),
        emp: materials.add(StandardMaterial {
            base_color: Color::srgb(0.4, 0.72, 1.0),
            unlit: true,
            ..default()
        }),
        torpedo: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.55, 0.25),
            unlit: true,
            ..default()
        }),
        ghost: materials.add(StandardMaterial {
            base_color: Color::srgba(0.4, 0.9, 0.7, 0.35),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
    };

    // Camera: a perspective view slightly above the plane. The far plane is
    // pushed out so the distant starfield is visible.
    let cam = data::feel::CameraFeel::default();
    commands.spawn((
        Camera3d::default(),
        Projection::from(PerspectiveProjection {
            far: 30_000.0,
            ..default()
        }),
        // Seeded from the compiled-in camera feel; `camera_orbit` moves it to
        // wherever the loaded file says on the first frame.
        Transform::from_xyz(0.0, -cam.distance, cam.height).looking_at(Vec3::ZERO, Vec3::Z),
        // Ambient fill is a per-camera component in Bevy 0.19.
        AmbientLight {
            color: Color::srgb(0.6, 0.7, 1.0),
            brightness: 320.0,
            ..default()
        },
        // The starfield cubemap wrapping the scene (see `render.rs`).
        space_skybox(&skybox),
        MainCamera,
    ));

    // A key light raking across the plane so the hull boxes read as solid.
    commands.spawn((
        DirectionalLight {
            illuminance: 9_000.0,
            ..default()
        },
        Transform::from_xyz(600.0, 400.0, 900.0).looking_at(Vec3::ZERO, Vec3::Z),
    ));

    // The star at the heart of the system: an animated surface + corona (see
    // `star.rs`). The two stations stay simple spheres.
    spawn_star(
        &mut commands,
        &mut meshes,
        &mut star_surface_materials,
        &mut star_halo_materials,
        Vec2::ZERO,
        120.0,
    );
    spawn_landmark(
        &mut commands,
        &game_meshes,
        &mut materials,
        Vec2::new(-700.0, 500.0),
        60.0,
        Color::srgb(0.55, 0.60, 0.72),
        false,
    );
    spawn_landmark(
        &mut commands,
        &game_meshes,
        &mut materials,
        Vec2::new(820.0, -420.0),
        44.0,
        Color::srgb(0.60, 0.45, 0.40),
        false,
    );

    // The distant starfield is now the skybox cubemap (see `render.rs`).

    // Microwarp destination preview, hidden until the pilot aims a warp.
    commands.spawn((
        Mesh3d(game_meshes.ship.clone()),
        MeshMaterial3d(game_materials.ghost.clone()),
        Transform::default(),
        Visibility::Hidden,
        MicrowarpGhost,
    ));

    commands.insert_resource(game_meshes);
    commands.insert_resource(game_materials);

    // Ships are not spawned here: they are authored. The `Loading` state lays
    // out the scenario once its data has arrived (see `session::await_data`).

    // A pool of off-screen enemy markers, hidden until needed.
    for _ in 0..EDGE_MARKER_COUNT {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(EDGE_MARKER_SIZE),
                height: Val::Px(EDGE_MARKER_SIZE),
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::WHITE),
            EdgeMarker,
        ));
    }

    let _ = bounds;
}

/// Spawn a spherical landmark (star/station/planet).
fn spawn_landmark(
    commands: &mut Commands,
    meshes: &GameMeshes,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec2,
    radius: f32,
    color: Color,
    glowing: bool,
) {
    let material = materials.add(StandardMaterial {
        base_color: color,
        unlit: glowing,
        perceptual_roughness: 0.9,
        ..default()
    });
    commands.spawn((
        Landmark { radius },
        Mesh3d(meshes.sphere.clone()),
        MeshMaterial3d(material),
        Transform::from_translation(pos.extend(0.0)).with_scale(Vec3::splat(radius)),
    ));
}
