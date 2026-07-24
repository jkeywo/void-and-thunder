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
//! Controls — aim with the mouse (or right stick). Aiming a broadside, torpedo
//! or microwarp dilates time (bullet-time) from a rechargeable aim battery.
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
use bevy::time::{Real, Virtual};
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use vt_sim::prelude::*;

mod audio;
use audio::SfxPlugin;

mod render;
use render::{space_skybox, SpaceSkyboxAsset, SpaceSkyboxPlugin};

mod star;
use star::{spawn_star, StarHaloMaterial, StarPlugin, StarSurfaceMaterial};

mod hud;
use hud::HudBridgePlugin;

// ---- Presentation constants ----

// Placeholder ship hulls: CC0 low-poly models from Quaternius's Ultimate
// Spaceships pack (see assets/CREDITS.md), converted to glb oriented bow-along-+X
// and normalised to ~44-unit length. Each faction gets a distinct hull plus the
// pack's matching colour variant, so faction identity comes from the textured
// paint while `damage_tint` only multiplies in the hull/brace/EMP state.
const SHIP_MODEL_PLAYER: &str = "models/executioner.glb";

/// The glb + colour-variant texture placeholder for a faction's hull.
fn ship_model(faction: &Faction) -> (&'static str, &'static str) {
    match faction {
        Faction::Corsairs => (
            "models/executioner.glb",
            "models/textures/executioner_green.png",
        ),
        Faction::Houses => ("models/imperial.glb", "models/textures/imperial_red.png"),
        Faction::Janissariat => ("models/bob.glb", "models/textures/bob_orange.png"),
        Faction::Guild => (
            "models/dispatcher.glb",
            "models/textures/dispatcher_blue.png",
        ),
        Faction::Freebooters => (
            "models/challenger.glb",
            "models/textures/challenger_purple.png",
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
/// How far behind the ship the camera sits.
const CAM_DISTANCE: f32 = 430.0;
/// How high above the plane the camera sits — low, so the view is a shallow
/// "slightly above, pointing slightly down" angle rather than top-down.
const CAM_HEIGHT: f32 = 170.0;
/// How quickly the camera yaw catches up to the ship's heading.
const CAM_YAW_LERP: f32 = 2.5;
/// Camera pitch (radians above horizontal): resting, minimum, maximum, and the
/// value it eases to while aiming a broadside. As pitch rises the camera is
/// raised and pulled in toward the ship (see `camera_orbit`).
const CAM_PITCH_BASE: f32 = 0.85;
const CAM_PITCH_MIN: f32 = 0.35;
const CAM_PITCH_MAX: f32 = 1.2;
const CAM_AIM_PITCH: f32 = 0.72;
/// How much the camera pulls in toward the ship at maximum pitch (0 = none).
const CAM_PITCH_ZOOM: f32 = 0.4;
/// Near-top-down pitch used while aiming a torpedo volley or a microwarp — the
/// camera rises directly over the ship and the aim becomes a top-down pointer.
const CAM_TOPDOWN_PITCH: f32 = 1.5;
/// How high the camera pulls out for the top-down (torpedo / microwarp) view, so
/// the whole tactical area around the ship is visible from overhead.
const CAM_TOPDOWN_DIST: f32 = 1500.0;
/// Faster yaw/pitch ease while locked to an aim (broadside / microwarp).
const CAM_AIM_LERP: f32 = 9.0;
/// How quickly the eased camera distance catches its target.
const CAM_DIST_LERP: f32 = 6.0;
/// Gamepad free-look rate (radians/sec of yaw, units/sec of pitch, at full
/// stick). The right stick moves the *view* like a mouse — deflection is a rate,
/// not an absolute offset — while not aiming a broadside.
const LOOK_YAW_RATE: f32 = 2.4;
const LOOK_PITCH_RATE: f32 = 1.8;
/// How far the gamepad free-look yaw may swing from dead-astern (radians) — a
/// full half-turn, so the view can be swung all the way to the ship's front.
const LOOK_YAW_LIMIT: f32 = PI;
/// Seconds of no look input before the camera eases back to its default trailing
/// position (behind the ship when moving forward, ahead of it when reversing).
const RECENTER_DELAY: f32 = 3.0;
/// How gently the idle camera eases back to its default position.
const RECENTER_LERP: f32 = 1.6;
/// Gamepad aim-pointer speed (world units/sec at full stick) for the top-down
/// torpedo / microwarp pointer and the EMP aim — rate-based, like a mouse.
const AIM_CURSOR_RATE: f32 = 780.0;
/// How far from the ship the gamepad aim pointer may stray.
const AIM_CURSOR_MAX: f32 = 1300.0;
/// Spacing between grid lines on the plane.
const GRID_SPACING: f32 = 200.0;
/// Grid cells each way.
const GRID_CELLS: u32 = 30;
/// The grid sits just below the plane so hulls float above it.
const GRID_Z: f32 = -9.0;
/// Length of the drawn aim beam while holding a broadside.
const AIM_BEAM_LEN: f32 = 620.0;
/// Size of the pool of off-screen enemy markers (max shown at once).
const EDGE_MARKER_COUNT: usize = 24;
/// Pixel size of an edge marker.
const EDGE_MARKER_SIZE: f32 = 16.0;
/// Inset of the edge markers from the window border, in pixels.
const EDGE_MARGIN: f32 = 42.0;

/// Marker for the entity the local player controls.
#[derive(Component)]
pub(crate) struct Player;

/// Marker for the camera so we can make it orbit the player.
#[derive(Component)]
struct MainCamera;

/// Marker for the heads-up text.
#[derive(Component)]
struct HudText;

/// Marker for the fill bar of the player's hull gauge.
#[derive(Component)]
struct HullBarFill;

/// One of the pooled UI markers that point at off-screen enemies.
#[derive(Component)]
struct EdgeMarker;

/// Shared meshes, built once at startup.
#[derive(Resource)]
struct GameMeshes {
    ship: Handle<Mesh>,
    /// Unit sphere — scaled per use.
    sphere: Handle<Mesh>,
    /// A long thin torpedo body (oriented along its velocity).
    torpedo: Handle<Mesh>,
}

/// Shared materials for things that never change colour.
#[derive(Resource)]
struct GameMaterials {
    shot: Handle<StandardMaterial>,
    emp: Handle<StandardMaterial>,
    torpedo: Handle<StandardMaterial>,
    ghost: Handle<StandardMaterial>,
}

/// The translucent preview of where a microwarp will drop the player.
#[derive(Component)]
struct MicrowarpGhost;

/// Render radius of a cannonball / EMP bolt (the shared sphere is unit-sized).
const SHOT_RADIUS: f32 = 7.0;

/// Which broadsides the player is currently *holding* (aiming). Purely
/// presentational — it drives the aim beam; firing happens on release.
#[derive(Resource, Default)]
struct Aiming {
    port: bool,
    starboard: bool,
}

/// Fraction of normal time while aiming (bullet-time).
const AIM_TIMESCALE: f32 = 0.10;

/// The aim battery: aiming a weapon dilates global time toward
/// [`AIM_TIMESCALE`] while this has charge, then eases back. Charge is spent in
/// real seconds (so the window is a real ~5s regardless of the dilation) and
/// recovers when not aiming. `dilation` is the current eased timescale.
#[derive(Resource)]
struct AimBattery {
    charge: f32,
    max: f32,
    drain_per_sec: f32,
    recharge_per_sec: f32,
    dilation: f32,
}

impl Default for AimBattery {
    fn default() -> Self {
        Self {
            charge: 5.0,
            max: 5.0,
            drain_per_sec: 1.0,
            recharge_per_sec: 1.0,
            dilation: 1.0,
        }
    }
}

/// The most recently used input device, so the HUD can show matching hints.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
enum InputMethod {
    #[default]
    KeyboardMouse,
    Gamepad,
}

/// Whether the player ship is currently flown by the AI (toggle with T). The
/// camera stays under the player's control either way.
#[derive(Resource, Default)]
struct PlayerAi {
    on: bool,
}

/// Whether the game is paused. Pausing freezes `Time<Virtual>` (and with it the
/// whole `FixedUpdate` simulation and every virtual-time visual); real-time
/// input still flows so the pause can be lifted.
#[derive(Resource, Default)]
struct Paused(bool);

/// Persistent free-look offset for gamepad camera control. The right stick nudges
/// these like a mouse (rate, not absolute); the camera yaw sits at
/// `heading + yaw_offset` and pitch at `pitch`. Mouse look stays absolute and
/// ignores this.
#[derive(Resource)]
struct FreeLook {
    yaw_offset: f32,
    pitch: f32,
    /// Seconds since the player last moved the look control. After
    /// [`RECENTER_DELAY`] the view eases back to its default trailing position.
    idle: f32,
    /// Last cursor position, to detect mouse look movement.
    last_cursor: Vec2,
}

impl Default for FreeLook {
    fn default() -> Self {
        Self {
            yaw_offset: 0.0,
            pitch: CAM_PITCH_BASE,
            idle: 0.0,
            last_cursor: Vec2::ZERO,
        }
    }
}

/// The world point the aim reticle sits on for the pointer-aimed kit (EMP,
/// torpedo, microwarp). The mouse sets it absolutely (plane pick); the gamepad
/// nudges it at a rate (like a mouse). It rests just ahead of the bow whenever
/// nothing is being aimed, so each aim starts from a sensible spot.
#[derive(Resource, Default)]
struct AimCursor {
    world: Vec2,
}

/// Marker for the boost-battery gauge fill.
#[derive(Component)]
struct BoostBarFill;

/// Marker for the aim-battery gauge fill.
#[derive(Component)]
struct AimBarFill;

/// Camera orbit + screen-shake state. `yaw`/`pitch` are eased toward targets set
/// by free-look or the active aim mode; `trauma` decays each frame and is added
/// to by hits and explosions.
#[derive(Resource)]
struct CameraRig {
    target: Vec2,
    yaw: f32,
    pitch: f32,
    /// Eased eye distance from the focus, so the top-down modes can pull the
    /// camera smoothly up and out.
    dist: f32,
    trauma: f32,
    seed: u32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: CAM_PITCH_BASE,
            dist: CAM_DISTANCE,
            trauma: 0.0,
            seed: 0,
        }
    }
}

impl CameraRig {
    fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount).clamp(0.0, 1.0);
    }

    /// Next pseudo-random float in `-1.0..1.0`.
    fn noise(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        ((self.seed >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    }
}

/// A short-lived visual effect (muzzle flash, hit spark, explosion) that scales
/// and fades out over its life, then despawns.
#[derive(Component)]
struct Effect {
    age: f32,
    life: f32,
    start_scale: f32,
    end_scale: f32,
    color: Color,
}

/// Wrap an angle to `(-PI, PI]`.
fn wrap_angle(angle: f32) -> f32 {
    let a = angle.rem_euclid(TAU);
    if a > PI {
        a - TAU
    } else {
        a
    }
}

/// Radial deadzone below which the stick reads zero, above which it is rescaled
/// so motion starts smoothly just past the edge (5% → ~0, 100% → 100%).
fn deadzone(v: f32) -> f32 {
    const DZ: f32 = 0.05;
    let a = v.abs();
    if a < DZ {
        0.0
    } else {
        v.signum() * (a - DZ) / (1.0 - DZ)
    }
}

/// Base display colour for a faction's ships (before damage tinting).
fn faction_color(faction: &Faction) -> Color {
    match faction {
        Faction::Corsairs => Color::srgb(0.35, 0.85, 0.55),
        Faction::Houses => Color::srgb(0.85, 0.30, 0.30),
        Faction::Janissariat => Color::srgb(0.85, 0.65, 0.20),
        Faction::Guild => Color::srgb(0.45, 0.60, 0.90),
        Faction::Freebooters => Color::srgb(0.75, 0.45, 0.85),
    }
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
        .set(ImagePlugin::default_nearest());
    // On the web, Bevy audio is disabled — sound goes through a WebAudio shim
    // (see src/audio.rs). Native keeps Bevy audio.
    #[cfg(target_arch = "wasm32")]
    let default_plugins = default_plugins.disable::<bevy::audio::AudioPlugin>();

    App::new()
        .add_plugins(default_plugins)
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .add_plugins(SimPlugin)
        .add_plugins(SfxPlugin)
        .add_plugins(SpaceSkyboxPlugin)
        .add_plugins(StarPlugin)
        .add_plugins(HudBridgePlugin)
        .init_state::<GameState>()
        .init_resource::<CameraRig>()
        .init_resource::<Aiming>()
        .init_resource::<AimBattery>()
        .init_resource::<InputMethod>()
        .init_resource::<PlayerAi>()
        .init_resource::<Paused>()
        .init_resource::<FreeLook>()
        .init_resource::<AimCursor>()
        .add_systems(Startup, setup)
        // Presentation runs in every state.
        .add_systems(
            Update,
            (
                attach_ship_visuals,
                attach_projectile_visuals,
                attach_empbolt_visuals,
                attach_torpedo_visuals,
                orient_torpedoes,
                damage_tint,
                camera_orbit,
                draw_grid,
                draw_aim_beams,
                draw_charge_telegraph,
                draw_reticle,
                draw_boarding,
                draw_microwarp_range,
                microwarp_ghost,
                update_hud,
                update_hull_bar,
                update_battery_bars,
                update_offscreen_markers,
                aim_time_dilation,
            ),
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
        .add_systems(
            Update,
            (player_input, watch_outcome, toggle_player_ai, toggle_pause)
                .run_if(in_state(GameState::Playing)),
        )
        .add_systems(Update, track_input_method)
        // Game over: wait for a restart.
        .add_systems(Update, restart.run_if(in_state(GameState::GameOver)))
        .run();
}

/// Whether a run is in progress or has ended (win or loss).
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
enum GameState {
    #[default]
    Playing,
    GameOver,
}

/// The player's starting position, offset from the star at the origin.
const PLAYER_START: Vec2 = Vec2::new(0.0, -520.0);

/// Spawn the player's corsair sloop — the encounter's protagonist.
fn spawn_player(commands: &mut Commands) {
    // A player ship is an ordinary ship — the full kit comes from the loadout;
    // the `Player` marker is what routes the client's input into its PilotIntent.
    commands.spawn((
        ship_bundle(
            Faction::Corsairs,
            ShipStats::default(),
            100.0,
            PLAYER_START,
            ShipLoadout::player(),
        ),
        Player,
        Protagonist,
    ));
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
    commands.spawn((
        Camera3d::default(),
        Projection::from(PerspectiveProjection {
            far: 30_000.0,
            ..default()
        }),
        Transform::from_xyz(0.0, -CAM_DISTANCE, CAM_HEIGHT).looking_at(Vec3::ZERO, Vec3::Z),
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

    // The player's corsair sloop. Enemy waves come from the sim's SpawnDirector.
    spawn_player(&mut commands);

    // Heads-up display.
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(20.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.88, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(14.0),
            ..default()
        },
        HudText,
    ));

    // Player hull gauge: a framed bar in the bottom-left whose fill tracks hull.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(16.0),
                left: Val::Px(16.0),
                width: Val::Px(240.0),
                height: Val::Px(18.0),
                padding: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.30, 0.34, 0.42)),
        ))
        .with_children(|frame| {
            frame.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.35, 0.85, 0.55)),
                HullBarFill,
            ));
        });

    // Boost battery (cyan) and aim battery (amber), stacked above the hull bar.
    spawn_bar(
        &mut commands,
        40.0,
        Color::srgb(0.35, 0.75, 0.95),
        BoostBarFill,
    );
    spawn_bar(
        &mut commands,
        60.0,
        Color::srgb(0.95, 0.8, 0.35),
        AimBarFill,
    );

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

/// Spawn a thin framed gauge in the bottom-left with a coloured fill, tagged
/// with a marker component so a system can resize it.
fn spawn_bar(commands: &mut Commands, bottom: f32, fill: Color, marker: impl Component) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(bottom),
                left: Val::Px(16.0),
                width: Val::Px(180.0),
                height: Val::Px(12.0),
                padding: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.20, 0.22, 0.28)),
        ))
        .with_children(|frame| {
            frame.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(fill),
                marker,
            ));
        });
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

/// Translate keyboard **and** gamepad into the player ship's helm, fire
/// requests, brace and boarding intent.
///
/// Broadsides are **hold to aim, release to fire**: while a side's button is
/// held we only show the aim beam; the release edge raises the sim's
/// [`FireOrders`] request, which `weapons_system` consumes exactly once.
#[allow(clippy::too_many_arguments)]
fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    real: Res<Time<Real>>,
    method: Res<InputMethod>,
    paused: Res<Paused>,
    player_ai: Res<PlayerAi>,
    mut board: ResMut<BoardIntent>,
    mut aiming: ResMut<Aiming>,
    mut aim_cursor: ResMut<AimCursor>,
    mut player: Query<
        (
            &mut Helm,
            &mut FireOrders,
            &mut Brace,
            &mut BoostDrive,
            &mut PilotIntent,
            &Transform,
            &Heading,
            &Broadside,
        ),
        With<Player>,
    >,
) {
    // Paused, or the AI is flying — the client stays hands-off (the camera is
    // handled separately). When the AI flies, the sim writes the ship's intent.
    if paused.0 || player_ai.on {
        return;
    }
    let Ok((mut helm, mut orders, mut brace, mut boost, mut pilot, transform, heading, bank)) =
        player.single_mut()
    else {
        return;
    };

    // --- Keyboard ---
    let mut throttle = 0.0;
    if keys.pressed(KeyCode::KeyW) {
        throttle += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        throttle -= 1.0;
    }

    let mut turn = 0.0;
    if keys.pressed(KeyCode::KeyA) {
        turn += 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        turn -= 1.0;
    }

    // Broadsides on the mouse buttons; EMP on Q.
    let mut aim_port = mouse.pressed(MouseButton::Left);
    let mut aim_starboard = mouse.pressed(MouseButton::Right);
    let mut fire_port = mouse.just_released(MouseButton::Left);
    let mut fire_starboard = mouse.just_released(MouseButton::Right);
    let mut emp_fire = keys.pressed(KeyCode::KeyQ);
    let mut torpedo_hold = keys.pressed(KeyCode::ControlLeft);
    let mut microwarp_hold = keys.pressed(KeyCode::ShiftLeft);
    let mut bracing = keys.pressed(KeyCode::KeyC);
    let mut boosting = keys.pressed(KeyCode::Space);
    let mut board_now = keys.just_pressed(KeyCode::KeyB);

    // --- Gamepad (first connected pad): the final scheme ---
    let pad = gamepads.iter().next();
    if let Some(pad) = pad {
        throttle += deadzone(pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0));
        // Stick right (+X) steers starboard (negative turn).
        turn -= deadzone(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0));

        aim_port |= pad.pressed(GamepadButton::LeftTrigger2); // LT
        aim_starboard |= pad.pressed(GamepadButton::RightTrigger2); // RT
        fire_port |= pad.just_released(GamepadButton::LeftTrigger2);
        fire_starboard |= pad.just_released(GamepadButton::RightTrigger2);
        torpedo_hold |= pad.pressed(GamepadButton::LeftTrigger); // LB
        microwarp_hold |= pad.pressed(GamepadButton::RightTrigger); // RB
        emp_fire |= pad.pressed(GamepadButton::West); // X / Square
        bracing |= pad.pressed(GamepadButton::North); // Y / Triangle
        boosting |= pad.pressed(GamepadButton::South); // A / Cross
        board_now |= pad.just_pressed(GamepadButton::East); // B / Circle
    }

    helm.throttle = throttle.clamp(-1.0, 1.0);
    helm.turn = turn.clamp(-1.0, 1.0);
    aiming.port = aim_port;
    aiming.starboard = aim_starboard;

    // --- Aim ---
    let ship = transform.translation.truncate();
    let dt = real.delta_secs();
    let use_pad = *method == InputMethod::Gamepad;
    let right_stick = |axis: GamepadAxis| pad.map_or(0.0, |p| deadzone(p.get(axis).unwrap_or(0.0)));
    // The mouse's plane pick, or `default` when the ray misses / off-screen.
    let mouse_plane = |default: Vec2| -> Vec2 {
        if let (Ok((camera, cam_gt)), Ok(window)) = (camera_q.single(), windows.single()) {
            if let Some(cursor) = window.cursor_position() {
                if let Ok(ray) = camera.viewport_to_world(cam_gt, cursor) {
                    if let Some(d) = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Z))
                    {
                        return ray.get_point(d).truncate();
                    }
                }
            }
        }
        default
    };

    let aiming_broadside = aim_port || aim_starboard;
    let aim_point = if aiming_broadside {
        // The yaw axis drives the aim *directly* across the bank's arc — full
        // deflection reaches the arc's edge, centre points straight out the beam.
        // (The camera yaw follows this, so you steer the whole view.) Prefer port
        // when both sides are held, matching the sim's volley choice.
        let axis = if use_pad {
            right_stick(GamepadAxis::RightStickX)
        } else if let Ok(window) = windows.single() {
            window
                .cursor_position()
                .map(|c| (c.x / window.width().max(1.0) * 2.0 - 1.0).clamp(-1.0, 1.0))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let is_port = aim_port;
        let beam = heading.0 + if is_port { PI * 0.5 } else { -PI * 0.5 };
        // Right (+axis) sweeps the aim clockwise (toward the bow on starboard,
        // toward the stern on port) — a consistent "push right, swing right".
        let angle = beam - axis * bank.arc;
        ship + Vec2::from_angle(angle) * 320.0
    } else if emp_fire || torpedo_hold || microwarp_hold {
        // Pointer aim for the kit. Mouse sets it absolutely (plane pick); the
        // gamepad nudges the persistent cursor at a rate, like a mouse.
        if use_pad {
            if let Ok((_, cam_gt)) = camera_q.single() {
                let right = cam_gt.right().truncate().normalize_or_zero();
                let up = cam_gt.up().truncate().normalize_or_zero();
                let sx = right_stick(GamepadAxis::RightStickX);
                let sy = right_stick(GamepadAxis::RightStickY);
                aim_cursor.world += (right * sx + up * sy) * AIM_CURSOR_RATE * dt;
            }
            let off = aim_cursor.world - ship;
            if off.length() > AIM_CURSOR_MAX {
                aim_cursor.world = ship + off.normalize_or_zero() * AIM_CURSOR_MAX;
            }
        } else {
            aim_cursor.world = mouse_plane(ship + heading.forward() * 320.0);
        }
        aim_cursor.world
    } else {
        // Idle: rest the pointer just ahead of the bow (the mouse still tracks the
        // plane on desktop so the reticle sits under the cursor).
        let rest = ship + heading.forward() * 320.0;
        aim_cursor.world = if use_pad { rest } else { mouse_plane(rest) };
        aim_cursor.world
    };

    // Only ever raise the request — the sim clears it once consumed, so a
    // release is never lost between fixed steps. The aim direction is already
    // within the arc, so the sim's clamp leaves it be.
    let aim_dir = (aim_point - ship).normalize_or_zero();
    if fire_port {
        orders.port = true;
        orders.aim = Some(aim_dir);
    }
    if fire_starboard {
        orders.starboard = true;
        orders.aim = Some(aim_dir);
    }
    brace.active = bracing;
    boost.active = boosting;
    pilot.aim_point = aim_point;
    pilot.emp_fire = emp_fire;
    pilot.torpedo_hold = torpedo_hold;
    pilot.microwarp_hold = microwarp_hold;
    if board_now {
        board.active = true;
    }
}

/// Give every ship without one its faction hull: a low-poly model textured with
/// the faction's colour variant. `base_color` starts white so the painted hull
/// shows through; `damage_tint` then multiplies it for hull/brace/EMP state.
fn attach_ship_visuals(
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
fn attach_projectile_visuals(
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
fn attach_empbolt_visuals(
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
fn attach_torpedo_visuals(
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
fn orient_torpedoes(mut torps: Query<(&Torpedo, &mut Transform)>) {
    for (torp, mut transform) in &mut torps {
        let dir = torp.vel.normalize_or(Vec3::Z);
        transform.rotation = Quat::from_rotation_arc(Vec3::Z, dir);
    }
}

/// Tint each ship's material — a multiplier over the faction-painted hull:
/// darker as its hull wears down, grey when crippled (boardable), a blue cast
/// while bracing, and a cyan EMP glow as it is disabled by EMP. Faction colour
/// lives in the texture now, so this only carries the ship's *state*.
fn damage_tint(
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

/// A blue crackle where an EMP bolt lands.
fn spawn_emp_effects(
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

/// Orbit the camera around the player. When aiming a broadside the view snaps to
/// the aim direction; otherwise the player free-looks with the mouse (offset
/// from screen centre) or right stick — yaw around the ship, pitch between
/// looking down at it and near-horizontal. Screen shake is added to the eye.
#[allow(clippy::too_many_arguments)]
fn camera_orbit(
    real: Res<Time<Real>>,
    paused: Res<Paused>,
    method: Res<InputMethod>,
    mut rig: ResMut<CameraRig>,
    mut freelook: ResMut<FreeLook>,
    aiming: Res<Aiming>,
    player_ai: Res<PlayerAi>,
    windows: Query<&Window>,
    gamepads: Query<&Gamepad>,
    player: Query<
        (
            &Transform,
            &Heading,
            &Velocity,
            &Broadside,
            &MicrowarpDrive,
            &PilotIntent,
        ),
        (With<Player>, Without<MainCamera>),
    >,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    // Camera runs on real time so it stays responsive during bullet-time; a pause
    // freezes it by zeroing the step.
    let dt = if paused.0 { 0.0 } else { real.delta_secs() };
    let use_pad = *method == InputMethod::Gamepad;

    // Mouse free-look is absolute (cursor offset from centre); gamepad free-look
    // is rate-based (the stick nudges a persistent offset, like a mouse) and is
    // integrated only in the free-look branch below, so it never drifts while the
    // same stick is steering an aim.
    let (mut look_x, mut look_y) = (0.0f32, 0.0f32);
    let mut look_active = false;
    if let Ok(window) = windows.single() {
        if let (Some(cursor), (w, h)) =
            (window.cursor_position(), (window.width(), window.height()))
        {
            look_x = ((cursor.x / w) * 2.0 - 1.0).clamp(-1.0, 1.0);
            look_y = ((cursor.y / h) * 2.0 - 1.0).clamp(-1.0, 1.0);
            // The mouse counts as look input only while it's actually moving.
            if !use_pad && cursor.distance(freelook.last_cursor) > 1.0 {
                look_active = true;
            }
            freelook.last_cursor = cursor;
        }
    }
    if use_pad {
        if let Some(pad) = gamepads.iter().next() {
            let sx = deadzone(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0));
            let sy = deadzone(pad.get(GamepadAxis::RightStickY).unwrap_or(0.0));
            look_active = sx != 0.0 || sy != 0.0;
        }
    }

    let (mut target_yaw, mut target_pitch, mut target_dist) =
        (rig.yaw, CAM_PITCH_BASE, CAM_DISTANCE);
    let mut desired_focus = rig.target;
    let mut locked = false;
    if let Ok((transform, heading, velocity, bank, drive, pilot)) = player.single() {
        let pos = transform.translation.truncate();
        desired_focus = pos;
        // While the AI flies, the camera stays a free-look — it doesn't snap to
        // the AI's aiming.
        let manual = !player_ai.on;
        let aiming_broadside = aiming.port || aiming.starboard;
        if manual && (pilot.microwarp_hold || pilot.torpedo_hold) {
            // Directly overhead and high up — a top-down pointer view. Microwarp
            // frames the (range-clamped) destination; torpedoes stay on the ship.
            if pilot.microwarp_hold {
                desired_focus = clamp_to_range(pos, pilot.aim_point, drive.range);
            }
            target_yaw = heading.0;
            target_pitch = CAM_TOPDOWN_PITCH;
            target_dist = CAM_TOPDOWN_DIST;
            locked = true;
        } else if manual && aiming_broadside {
            // Lock the yaw along where the broadside points; the aim axis steers
            // it across the arc (see `player_input`).
            let is_port = aiming.port;
            let aim_dir = (pilot.aim_point - pos).normalize_or_zero();
            let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
            target_yaw = dir.to_angle();
            target_pitch = CAM_AIM_PITCH;
            target_dist = CAM_DISTANCE * (1.0 - CAM_PITCH_ZOOM);
            locked = true;
        } else if use_pad {
            // Gamepad free-look: integrate the right stick into a persistent
            // offset (rate, like a mouse), then sit the view at heading + offset.
            if let Some(pad) = gamepads.iter().next() {
                let sx = deadzone(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0));
                let sy = deadzone(pad.get(GamepadAxis::RightStickY).unwrap_or(0.0));
                freelook.yaw_offset = (freelook.yaw_offset - sx * LOOK_YAW_RATE * dt)
                    .clamp(-LOOK_YAW_LIMIT, LOOK_YAW_LIMIT);
                freelook.pitch = (freelook.pitch - sy * LOOK_PITCH_RATE * dt)
                    .clamp(CAM_PITCH_MIN, CAM_PITCH_MAX);
            }
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        } else {
            // Mouse free-look: absolute offset from the heading (yaw not inverted).
            target_yaw = heading.0 - look_x * 0.9;
            target_pitch = (CAM_PITCH_BASE + look_y * 0.5).clamp(CAM_PITCH_MIN, CAM_PITCH_MAX);
        }

        // Idle auto-recenter: after a few seconds without look input, ease the
        // view back to its default trailing position — behind the ship when
        // making way ahead, ahead of it (looking back) when reversing. Aiming
        // counts as activity, so the timer restarts once an aim ends.
        if locked || look_active {
            freelook.idle = 0.0;
        } else {
            freelook.idle += dt;
        }
        if !locked && freelook.idle > RECENTER_DELAY {
            let forward = heading.forward();
            let reversing = velocity.0.dot(forward) < -5.0;
            let default_off = if reversing { PI } else { 0.0 };
            let rk = 1.0 - (-RECENTER_LERP * dt).exp();
            freelook.yaw_offset += wrap_angle(default_off - freelook.yaw_offset) * rk;
            freelook.pitch += (CAM_PITCH_BASE - freelook.pitch) * rk;
            target_yaw = heading.0 + freelook.yaw_offset;
            target_pitch = freelook.pitch;
        }
    }

    let lerp = if locked { CAM_AIM_LERP } else { CAM_YAW_LERP };
    let k = 1.0 - (-lerp * dt).exp();
    rig.yaw = wrap_angle(rig.yaw + wrap_angle(target_yaw - rig.yaw) * k);
    rig.pitch += (target_pitch - rig.pitch) * k;
    let dk = 1.0 - (-CAM_DIST_LERP * dt).exp();
    rig.dist += (target_dist - rig.dist) * dk;
    // Ease the focus toward the ship (or the warp point) tightly.
    let fk = 1.0 - (-10.0 * dt).exp();
    let focus_step = (desired_focus - rig.target) * fk;
    rig.target += focus_step;

    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - dt * 1.4).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let shake = Vec3::new(rig.noise(), rig.noise(), rig.noise() * 0.5) * 26.0 * amount;

    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    // As pitch rises the camera lifts (sin) and swings overhead (cos shrinks the
    // horizontal reach); the eased distance sets how far out the eye sits.
    let look = Vec2::from_angle(rig.yaw);
    let back = Vec3::new(-look.x, -look.y, 0.0);
    let focus = rig.target.extend(0.0);
    let eye = focus + (back * rig.pitch.cos() + Vec3::Z * rig.pitch.sin()) * rig.dist + shake;
    *camera = Transform::from_translation(eye).looking_at(focus, Vec3::Z);
}

/// Draw the reference grid on the plane, plus the system boundary ring.
fn draw_grid(mut gizmos: Gizmos, bounds: Res<SystemBounds>) {
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
fn draw_aim_beams(
    mut gizmos: Gizmos,
    aiming: Res<Aiming>,
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
        for shot in broadside_volley(pos, velocity.0, dir, bank) {
            let beam = shot.velocity.normalize_or_zero();
            let start = shot.position.extend(0.0);
            let end = (shot.position + beam * AIM_BEAM_LEN).extend(0.0);
            gizmos.line(start, end, color);
        }
    }
}

/// Draw the boarding prompt: a ring around the crippled ship you're holding
/// alongside, filling clockwise as the dwell completes. It appears only while
/// the protagonist is in range (the sim sets [`Boarding::target`] then), giving
/// the "close enough to board" cue.
fn draw_boarding(mut gizmos: Gizmos, boarding: Res<Boarding>, ships: Query<&Transform>) {
    let Some(target) = boarding.target else {
        return;
    };
    let Ok(tf) = ships.get(target) else {
        return;
    };
    let pos = tf.translation.truncate();
    let frac = (boarding.progress / BOARD_DWELL).clamp(0.0, 1.0);
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

/// Draw a small reticle on the plane where the aim cursor points.
fn draw_reticle(mut gizmos: Gizmos, player: Query<&PilotIntent, With<Player>>) {
    let Ok(pilot) = player.single() else {
        return;
    };
    gizmos.circle(
        Isometry3d::from_translation(pilot.aim_point.extend(1.0)),
        14.0,
        Color::srgba(1.0, 1.0, 1.0, 0.5),
    );
}

/// While placing a microwarp, draw the reachable range ring and a line to the
/// clamped destination.
fn draw_microwarp_range(
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

/// Show the microwarp ghost at the clamped destination while the pilot aims a
/// warp, matching the player's heading; hide it otherwise.
fn microwarp_ghost(
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
fn draw_charge_telegraph(mut gizmos: Gizmos, ships: Query<(&Transform, &Broadside)>) {
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
fn muzzle_flashes(
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

/// Sparks and a little shake when a hull is hit.
fn spawn_hit_effects(
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
fn spawn_destroy_effects(
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
fn update_effects(
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

/// Resize the hull gauge to the player's remaining hull (and recolour it).
fn update_hull_bar(
    player: Query<&Hull, With<Player>>,
    mut fill: Query<(&mut Node, &mut BackgroundColor), With<HullBarFill>>,
) {
    let frac = player
        .single()
        .map(|hull| (hull.current / hull.max).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    for (mut node, mut color) in &mut fill {
        node.width = Val::Percent(frac * 100.0);
        // Green when healthy, sliding to red as it drops.
        *color = BackgroundColor(Color::srgb(0.9 - 0.55 * frac, 0.3 + 0.55 * frac, 0.35));
    }
}

/// Point a pooled marker at each off-screen enemy, pinned to the window edge in
/// the enemy's on-screen direction. Uses the camera's own basis, so it stays
/// correct as the camera yaws.
fn update_offscreen_markers(
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    player_q: Query<&Transform, With<Player>>,
    enemies: Query<(&Transform, &Faction, Option<&Disabled>), (With<Ship>, Without<Player>)>,
    mut markers: Query<(&mut Node, &mut BackgroundColor), With<EdgeMarker>>,
) {
    let hide_all = |markers: &mut Query<(&mut Node, &mut BackgroundColor), With<EdgeMarker>>| {
        for (mut node, _) in markers.iter_mut() {
            node.display = Display::None;
        }
    };

    let (Ok(window), Ok((camera, cam_gt)), Ok(player)) =
        (windows.single(), camera_q.single(), player_q.single())
    else {
        hide_all(&mut markers);
        return;
    };

    let (w, h) = (window.width(), window.height());
    let center = Vec2::new(w * 0.5, h * 0.5);
    let ship = player.translation.truncate();
    // World directions that map to screen right / screen up.
    let screen_right = cam_gt.right().truncate().normalize_or_zero();
    let screen_up = cam_gt.up().truncate().normalize_or_zero();

    let mut pool = markers.iter_mut();
    for (enemy, faction, disabled) in &enemies {
        let on_screen = match camera.world_to_viewport(cam_gt, enemy.translation) {
            Ok(vp) => vp.x >= 0.0 && vp.x <= w && vp.y >= 0.0 && vp.y <= h,
            Err(_) => false, // behind the camera
        };
        if on_screen {
            continue;
        }

        let Some((mut node, mut color)) = pool.next() else {
            break; // pool exhausted; rare
        };

        let d = enemy.translation.truncate() - ship;
        // Screen-space direction (y is down in UI space).
        let dir = Vec2::new(d.dot(screen_right), -d.dot(screen_up)).normalize_or_zero();
        if dir == Vec2::ZERO {
            node.display = Display::None;
            continue;
        }
        // Intersect the ray from centre with the inset window rectangle.
        let (hw, hh) = (w * 0.5 - EDGE_MARGIN, h * 0.5 - EDGE_MARGIN);
        let tx = if dir.x.abs() > 1e-3 {
            hw / dir.x.abs()
        } else {
            f32::INFINITY
        };
        let ty = if dir.y.abs() > 1e-3 {
            hh / dir.y.abs()
        } else {
            f32::INFINITY
        };
        let pos = center + dir * tx.min(ty);

        node.display = Display::Flex;
        node.left = Val::Px(pos.x - EDGE_MARKER_SIZE * 0.5);
        node.top = Val::Px(pos.y - EDGE_MARKER_SIZE * 0.5);
        *color = BackgroundColor(if disabled.is_some() {
            Color::srgb(0.55, 0.58, 0.65) // crippled: boardable
        } else {
            faction_color(faction)
        });
    }

    // Hide any markers left unused this frame.
    for (mut node, _) in pool {
        node.display = Display::None;
    }
}

/// Dilate global time toward [`AIM_TIMESCALE`] while the player is aiming and the
/// aim battery has charge; ease back otherwise. Battery + easing run on real
/// time so the window is a real ~5s and stays smooth under dilation.
fn aim_time_dilation(
    real: Res<Time<Real>>,
    mut virt: ResMut<Time<Virtual>>,
    paused: Res<Paused>,
    aiming: Res<Aiming>,
    player: Query<&PilotIntent, With<Player>>,
    mut battery: ResMut<AimBattery>,
) {
    // While paused the virtual clock is frozen; leave the battery untouched and
    // don't fight the pause with a speed change.
    if paused.0 {
        return;
    }
    let dt = real.delta_secs();
    let (torpedo_hold, microwarp_hold) = player
        .single()
        .map(|p| (p.torpedo_hold, p.microwarp_hold))
        .unwrap_or((false, false));
    let wants_dilation = aiming.port || aiming.starboard || torpedo_hold || microwarp_hold;
    let target = if wants_dilation && battery.charge > 0.0 {
        battery.charge = (battery.charge - battery.drain_per_sec * dt).max(0.0);
        AIM_TIMESCALE
    } else {
        battery.charge = (battery.charge + battery.recharge_per_sec * dt).min(battery.max);
        1.0
    };
    let k = 1.0 - (-10.0 * dt).exp();
    battery.dilation += (target - battery.dilation) * k;
    virt.set_relative_speed(battery.dilation.max(0.02));
}

/// Resize the boost and aim battery gauges.
fn update_battery_bars(
    battery: Res<AimBattery>,
    player: Query<&BoostDrive, With<Player>>,
    mut boost_fill: Query<&mut Node, (With<BoostBarFill>, Without<AimBarFill>)>,
    mut aim_fill: Query<&mut Node, (With<AimBarFill>, Without<BoostBarFill>)>,
) {
    let boost_frac = player
        .single()
        .map(|b| (b.battery / b.battery_max).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    if let Ok(mut node) = boost_fill.single_mut() {
        node.width = Val::Percent(boost_frac * 100.0);
    }
    let aim_frac = (battery.charge / battery.max).clamp(0.0, 1.0);
    if let Ok(mut node) = aim_fill.single_mut() {
        node.width = Val::Percent(aim_frac * 100.0);
    }
}

/// Toggle AI control of the player ship with `T`. Enabling it drops an
/// `AiController` (piloting preset) onto the player so the sim's AI flies it;
/// disabling it removes the controller and hands the ship back. The camera is
/// never affected — the player always steers the view.
fn toggle_player_ai(
    keys: Res<ButtonInput<KeyCode>>,
    mut player_ai: ResMut<PlayerAi>,
    mut aiming: ResMut<Aiming>,
    mut commands: Commands,
    player: Query<Entity, With<Player>>,
) {
    if !keys.just_pressed(KeyCode::KeyT) {
        return;
    }
    player_ai.on = !player_ai.on;
    let Ok(entity) = player.single() else {
        return;
    };
    if player_ai.on {
        commands.entity(entity).insert(AiController::piloting());
        *aiming = Aiming::default(); // clear any held broadside aim
    } else {
        commands.entity(entity).remove::<AiController>();
    }
}

/// Toggle pause with `P` / `Escape` (or the pad's Start). Pausing freezes
/// `Time<Virtual>`, which halts the whole `FixedUpdate` simulation and every
/// virtual-time visual at once; real-time input keeps flowing so the pause can
/// be lifted.
fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut paused: ResMut<Paused>,
    mut virt: ResMut<Time<Virtual>>,
) {
    let pad_toggle = gamepads
        .iter()
        .any(|pad| pad.just_pressed(GamepadButton::Start));
    if !keys.just_pressed(KeyCode::KeyP) && !keys.just_pressed(KeyCode::Escape) && !pad_toggle {
        return;
    }
    paused.0 = !paused.0;
    if paused.0 {
        virt.pause();
    } else {
        virt.unpause();
    }
}

/// Track the last-used input device so the HUD shows matching control hints.
fn track_input_method(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut method: ResMut<InputMethod>,
) {
    let axes = [
        GamepadAxis::LeftStickX,
        GamepadAxis::LeftStickY,
        GamepadAxis::RightStickX,
        GamepadAxis::RightStickY,
    ];
    let buttons = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::LeftTrigger,
        GamepadButton::RightTrigger,
        GamepadButton::LeftTrigger2,
        GamepadButton::RightTrigger2,
        GamepadButton::Start,
    ];
    let pad_active = gamepads.iter().any(|pad| {
        axes.iter().any(|a| pad.get(*a).unwrap_or(0.0).abs() > 0.3)
            || buttons.iter().any(|b| pad.pressed(*b))
    });
    if pad_active {
        *method = InputMethod::Gamepad;
    } else if keys.get_pressed().next().is_some() || mouse.get_pressed().next().is_some() {
        *method = InputMethod::KeyboardMouse;
    }
}

/// Move to the game-over state once the encounter has resolved.
fn watch_outcome(encounter: Res<Encounter>, mut next: ResMut<NextState<GameState>>) {
    if encounter.outcome != Outcome::InProgress {
        next.set(GameState::GameOver);
    }
}

/// On the game-over screen, `R` (or the pad's Start) clears the field and starts
/// a fresh run.
fn restart(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut commands: Commands,
    ships: Query<Entity, With<Ship>>,
    projectiles: Query<Entity, With<Projectile>>,
    mut director: ResMut<SpawnDirector>,
    mut encounter: ResMut<Encounter>,
    mut plunder: ResMut<Plunder>,
    mut board: ResMut<BoardIntent>,
    mut next: ResMut<NextState<GameState>>,
) {
    let pad_restart = gamepads
        .iter()
        .any(|pad| pad.just_pressed(GamepadButton::Start));
    if !keys.just_pressed(KeyCode::KeyR) && !pad_restart {
        return;
    }
    for entity in ships.iter().chain(&projectiles) {
        commands.entity(entity).despawn();
    }
    reset_encounter(&mut director, &mut encounter);
    *plunder = Plunder::default();
    *board = BoardIntent::default();
    spawn_player(&mut commands);
    next.set(GameState::Playing);
}

/// Update the heads-up text with the wave, enemies left, plunder, and outcome.
fn update_hud(
    encounter: Res<Encounter>,
    plunder: Res<Plunder>,
    method: Res<InputMethod>,
    player_ai: Res<PlayerAi>,
    paused: Res<Paused>,
    boarding: Res<Boarding>,
    torps: Query<&TorpedoBay, With<Player>>,
    mut hud: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    let pause_line = if paused.0 {
        "‖ PAUSED — press P / Start to resume\n"
    } else {
        ""
    };
    let boarding_line = if boarding.target.is_some() {
        let pct = (boarding.progress / BOARD_DWELL * 100.0).clamp(0.0, 100.0) as u32;
        format!("◎ BOARDING {pct}% — hold position alongside the hulk\n")
    } else {
        String::new()
    };
    let ai_line = if player_ai.on {
        "◆ AI PILOT ENGAGED — you keep the camera · [T] resume control\n"
    } else {
        ""
    };
    let torp_line = torps
        .single()
        .map(|bay| {
            if bay.locks > 0 {
                format!(
                    "torpedoes: {} loaded  ·  LOCKED x{}",
                    bay.loaded as u32, bay.locks
                )
            } else {
                format!("torpedoes: {} loaded", bay.loaded as u32)
            }
        })
        .unwrap_or_default();
    let hints = match *method {
        InputMethod::KeyboardMouse => "[LMB/RMB] broadside  [Q] EMP  [Ctrl] torpedoes  [Shift] microwarp  [Space] boost  [C] brace  [P] pause  ·  board: hold alongside a hulk",
        InputMethod::Gamepad => "[LT/RT] broadside  [X] EMP  [LB] torpedoes  [RB] microwarp  [A] boost  [Y] brace  [Start] pause  ·  board: hold alongside a hulk",
    };
    text.0 = match encounter.outcome {
        Outcome::InProgress => format!(
            "{pause_line}{ai_line}{boarding_line}Wave {}  ·  enemies: {}  ·  plundered: {}  ·  {torp_line}\n{hints}  ·  [T] AI pilot",
            encounter.wave.max(1),
            encounter.enemies_remaining,
            plunder.ships_boarded,
        ),
        Outcome::Cleared => format!(
            "ALL {} WAVES CLEARED — the lanes are yours.  Ships plundered: {}.\nPress R to sail again.",
            encounter.wave, plunder.ships_boarded,
        ),
        Outcome::PlayerDestroyed => {
            "YOUR SHIP IS LOST TO THE VOID.\nPress R to sail again.".to_string()
        }
    };
}
