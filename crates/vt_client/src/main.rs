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
//! Controls (kit is being rolled out phase by phase — see the plan):
//!   W / S   — throttle forward / reverse
//!   A / D   — turn to port / starboard
//!   Q / E   — **hold** to aim the port / starboard broadside, **release** to fire
//!             (aiming dilates time — bullet-time — from the aim battery)
//!   Space   — boost (rechargeable battery)
//!   C       — brace (cut incoming damage)
//!   B       — board a crippled enemy alongside (loot it)
//!   R       — restart after a run ends
//!
//! Gamepad: RT/LT throttle & reverse, left stick steer, LB/RB hold-aim/
//! release-fire broadsides, A boost, Y brace, B board, Start restart.

use bevy::prelude::*;
use bevy::time::{Real, Virtual};
use std::f32::consts::{PI, TAU};
use vt_sim::prelude::*;

mod audio;
use audio::SfxPlugin;

// ---- Presentation constants ----

/// Hull box: length along the bow (+X), width, height.
const SHIP_SIZE: Vec3 = Vec3::new(44.0, 20.0, 13.0);
/// How far behind the ship the camera sits.
const CAM_DISTANCE: f32 = 430.0;
/// How high above the plane the camera sits — low, so the view is a shallow
/// "slightly above, pointing slightly down" angle rather than top-down.
const CAM_HEIGHT: f32 = 170.0;
/// How quickly the camera yaw catches up to the ship's heading.
const CAM_YAW_LERP: f32 = 2.5;
/// Camera pitch (radians above horizontal): resting, minimum, maximum, and the
/// value it eases to while aiming a broadside.
const CAM_PITCH_BASE: f32 = 0.55;
const CAM_PITCH_MIN: f32 = 0.28;
const CAM_PITCH_MAX: f32 = 1.15;
const CAM_AIM_PITCH: f32 = 0.72;
/// Spacing between grid lines on the plane.
const GRID_SPACING: f32 = 200.0;
/// Grid cells each way.
const GRID_CELLS: u32 = 30;
/// The grid sits just below the plane so hulls float above it.
const GRID_Z: f32 = -9.0;
/// Deadzone below which stick input is ignored.
const STICK_DEADZONE: f32 = 0.15;
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
struct Player;

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
}

/// Shared materials for things that never change colour.
#[derive(Resource)]
struct GameMaterials {
    shot: Handle<StandardMaterial>,
    star: Handle<StandardMaterial>,
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
const AIM_TIMESCALE: f32 = 0.25;

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
    trauma: f32,
    seed: u32,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            target: Vec2::ZERO,
            yaw: 0.0,
            pitch: CAM_PITCH_BASE,
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
        .init_state::<GameState>()
        .init_resource::<CameraRig>()
        .init_resource::<Aiming>()
        .init_resource::<AimBattery>()
        .add_systems(Startup, setup)
        // Presentation runs in every state.
        .add_systems(
            Update,
            (
                attach_ship_visuals,
                attach_projectile_visuals,
                attach_empbolt_visuals,
                attach_torpedo_visuals,
                damage_tint,
                camera_orbit,
                draw_grid,
                draw_aim_beams,
                draw_charge_telegraph,
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
            (player_input, watch_outcome).run_if(in_state(GameState::Playing)),
        )
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
    commands.spawn((
        ship_bundle(
            Faction::Corsairs,
            ShipStats::default(),
            100.0,
            PLAYER_START,
            Broadside::default(),
        ),
        Player,
        Protagonist,
        BoostDrive::default(),
        EmpWeapon::default(),
        TorpedoBay::default(),
        MicrowarpDrive::default(),
    ));
}

fn setup(
    mut commands: Commands,
    bounds: Res<SystemBounds>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Shared meshes and materials.
    let game_meshes = GameMeshes {
        ship: meshes.add(Cuboid::new(SHIP_SIZE.x, SHIP_SIZE.y, SHIP_SIZE.z)),
        sphere: meshes.add(Sphere::new(1.0)),
    };
    let game_materials = GameMaterials {
        shot: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.9, 0.4),
            unlit: true,
            ..default()
        }),
        star: materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.88, 1.0),
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

    // The star at the heart of the system, plus a couple of stations, as spheres.
    spawn_landmark(
        &mut commands,
        &game_meshes,
        &mut materials,
        Vec2::ZERO,
        120.0,
        Color::srgb(1.0, 0.82, 0.42),
        true,
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

    // A distant starfield shell. In 2.5D the parallax comes free from the
    // perspective camera, so these are just points far out in every direction.
    let mut rng = Lcg::new(0x5EED_C0DE);
    for _ in 0..400 {
        // Random direction on a sphere.
        let theta = rng.unit() * TAU;
        let z = rng.unit() * 2.0 - 1.0;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let dir = Vec3::new(r * theta.cos(), r * theta.sin(), z);
        let dist = 3_000.0 + rng.unit() * 3_000.0;
        let size = 10.0 + rng.unit() * 22.0;
        commands.spawn((
            Mesh3d(game_meshes.sphere.clone()),
            MeshMaterial3d(game_materials.star.clone()),
            Transform::from_translation(dir * dist).with_scale(Vec3::splat(size)),
        ));
    }

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

/// A tiny linear-congruential RNG — enough to scatter a starfield without
/// pulling in a dependency.
struct Lcg(u32);

impl Lcg {
    fn new(seed: u32) -> Self {
        Self(seed | 1)
    }

    /// Next float in `0.0..1.0`.
    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
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
    mut board: ResMut<BoardIntent>,
    mut aiming: ResMut<Aiming>,
    mut pilot: ResMut<PilotIntent>,
    mut player: Query<
        (
            &mut Helm,
            &mut FireOrders,
            &mut Brace,
            &mut BoostDrive,
            &Transform,
            &Heading,
        ),
        With<Player>,
    >,
) {
    let Ok((mut helm, mut orders, mut brace, mut boost, transform, heading)) = player.single_mut()
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

    // Aim cursor: mouse → plane (desktop), overridden by right stick when deflected.
    let ship = transform.translation.truncate();
    let mut aim_point = ship + heading.forward() * 320.0;
    if let (Ok((camera, cam_gt)), Ok(window)) = (camera_q.single(), windows.single()) {
        if let Some(cursor) = window.cursor_position() {
            if let Ok(ray) = camera.viewport_to_world(cam_gt, cursor) {
                if let Some(dist) = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Dir3::Z)) {
                    aim_point = ray.get_point(dist).truncate();
                }
            }
        }
        if let Some(pad) = gamepads.iter().next() {
            let stick = Vec2::new(
                pad.get(GamepadAxis::RightStickX).unwrap_or(0.0),
                pad.get(GamepadAxis::RightStickY).unwrap_or(0.0),
            );
            if stick.length() > STICK_DEADZONE {
                let right = cam_gt.right().truncate().normalize_or_zero();
                let up = cam_gt.up().truncate().normalize_or_zero();
                aim_point = ship + (right * stick.x + up * stick.y) * 520.0;
            }
        }
    }

    // --- Gamepad (first connected pad): the final scheme ---
    if let Some(pad) = gamepads.iter().next() {
        let ly = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if ly.abs() > STICK_DEADZONE {
            throttle += ly;
        }
        let lx = pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        if lx.abs() > STICK_DEADZONE {
            // Stick right (+X) steers starboard (negative turn).
            turn -= lx;
        }

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
    // Only ever raise the request — the sim clears it once consumed, so a
    // release is never lost between fixed steps. The aim direction (toward the
    // cursor) is clamped to the bank's arc by the sim.
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

/// Give every ship without one a hull box, coloured by faction.
fn attach_ship_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<(Entity, &Faction), (With<Ship>, Without<Mesh3d>)>,
) {
    for (entity, faction) in &ships {
        let material = materials.add(StandardMaterial {
            base_color: faction_color(faction),
            perceptual_roughness: 0.7,
            ..default()
        });
        commands
            .entity(entity)
            .insert((Mesh3d(meshes.ship.clone()), MeshMaterial3d(material)));
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

/// Give every torpedo an orange sphere (kept scaled by the homing system's
/// transform, which only overwrites translation).
fn attach_torpedo_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    mut torps: Query<(Entity, &mut Transform), (With<Torpedo>, Without<Mesh3d>)>,
) {
    for (entity, mut transform) in &mut torps {
        transform.scale = Vec3::splat(9.0);
        commands.entity(entity).insert((
            Mesh3d(meshes.sphere.clone()),
            MeshMaterial3d(mats.torpedo.clone()),
        ));
    }
}

/// Tint each ship's material: darker as its hull wears down, grey when crippled
/// (boardable), a blue cast while bracing, and a cyan EMP glow as it is disabled
/// by EMP.
fn damage_tint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<
        (
            &Faction,
            &Hull,
            &EmpDefense,
            &MeshMaterial3d<StandardMaterial>,
            Option<&Disabled>,
            Option<&Brace>,
        ),
        With<Ship>,
    >,
) {
    for (faction, hull, emp, material, disabled, brace) in &ships {
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };
        if disabled.is_some() {
            // Crippled hulk — drifting, boardable.
            material.base_color = Color::srgb(0.42, 0.44, 0.5);
            continue;
        }
        let frac = (hull.current / hull.max).clamp(0.0, 1.0);
        let k = 0.4 + 0.6 * frac;
        let base = faction_color(faction).to_srgba();
        let (mut r, mut g, mut b) = (base.red * k, base.green * k, base.blue * k);
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
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
    aiming: Res<Aiming>,
    pilot: Res<PilotIntent>,
    windows: Query<&Window>,
    gamepads: Query<&Gamepad>,
    player: Query<(&Transform, &Heading, &Broadside), (With<Player>, Without<MainCamera>)>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs();

    // Free-look offsets from the mouse (offset from centre) and right stick.
    let (mut look_x, mut look_y) = (0.0f32, 0.0f32);
    if let Ok(window) = windows.single() {
        if let (Some(cursor), (w, h)) =
            (window.cursor_position(), (window.width(), window.height()))
        {
            look_x = ((cursor.x / w) * 2.0 - 1.0).clamp(-1.0, 1.0);
            look_y = ((cursor.y / h) * 2.0 - 1.0).clamp(-1.0, 1.0);
        }
    }
    if let Some(pad) = gamepads.iter().next() {
        let sx = pad.get(GamepadAxis::RightStickX).unwrap_or(0.0);
        let sy = pad.get(GamepadAxis::RightStickY).unwrap_or(0.0);
        if sx.abs() > STICK_DEADZONE {
            look_x = sx;
        }
        if sy.abs() > STICK_DEADZONE {
            look_y = -sy;
        }
    }

    let (mut target_yaw, mut target_pitch) = (rig.yaw, CAM_PITCH_BASE);
    if let Ok((transform, heading, bank)) = player.single() {
        rig.target = transform.translation.truncate();
        let pos = transform.translation.truncate();
        let aiming_broadside = aiming.port || aiming.starboard;
        if pilot.microwarp_hold {
            // Swoop up to a near-top-down view to place the warp.
            target_yaw = heading.0;
            target_pitch = CAM_PITCH_MAX;
        } else if aiming_broadside {
            // Snap toward where the broadside points.
            let is_port = aiming.port;
            let aim_dir = (pilot.aim_point - pos).normalize_or_zero();
            let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
            target_yaw = dir.to_angle();
            target_pitch = CAM_AIM_PITCH;
        } else {
            // Follow the heading, offset by free-look.
            target_yaw = heading.0 + look_x * 0.9;
            target_pitch = (CAM_PITCH_BASE - look_y * 0.5).clamp(CAM_PITCH_MIN, CAM_PITCH_MAX);
        }
    }

    let k = 1.0 - (-CAM_YAW_LERP * dt).exp();
    rig.yaw = wrap_angle(rig.yaw + wrap_angle(target_yaw - rig.yaw) * k);
    rig.pitch += (target_pitch - rig.pitch) * k;

    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - dt * 1.4).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let shake = Vec3::new(rig.noise(), rig.noise(), rig.noise() * 0.5) * 26.0 * amount;

    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    let look = Vec2::from_angle(rig.yaw);
    let back = Vec3::new(-look.x, -look.y, 0.0);
    let focus = rig.target.extend(0.0);
    let eye = focus + (back * rig.pitch.cos() + Vec3::Z * rig.pitch.sin()) * CAM_DISTANCE + shake;
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
/// what actually fires.
fn draw_aim_beams(
    mut gizmos: Gizmos,
    aiming: Res<Aiming>,
    pilot: Res<PilotIntent>,
    player: Query<(&Transform, &Heading, &Velocity, &Broadside), With<Player>>,
) {
    let Ok((transform, heading, velocity, bank)) = player.single() else {
        return;
    };
    let pos = transform.translation.truncate();
    let aim_dir = (pilot.aim_point - pos).normalize_or_zero();
    let color = Color::srgba(1.0, 0.85, 0.4, 0.5);

    for (active, is_port) in [(aiming.port, true), (aiming.starboard, false)] {
        if !active {
            continue;
        }
        let dir = broadside_direction(heading.0, is_port, Some(aim_dir), bank.arc);
        for shot in broadside_volley(pos, velocity.0, dir, bank) {
            let beam = shot.velocity.normalize_or_zero();
            let start = shot.position.extend(0.0);
            let end = (shot.position + beam * AIM_BEAM_LEN).extend(0.0);
            gizmos.line(start, end, color);
        }
    }
}

/// Show the microwarp ghost at the clamped destination while the pilot aims a
/// warp, matching the player's heading; hide it otherwise.
fn microwarp_ghost(
    pilot: Res<PilotIntent>,
    player: Query<(&Transform, &Heading, &MicrowarpDrive), (With<Player>, Without<MicrowarpGhost>)>,
    mut ghost: Query<(&mut Transform, &mut Visibility), With<MicrowarpGhost>>,
) {
    let Ok((mut ghost_tf, mut visibility)) = ghost.single_mut() else {
        return;
    };
    if pilot.microwarp_hold {
        if let Ok((transform, heading, drive)) = player.single() {
            let origin = transform.translation.truncate();
            let dest = clamp_to_range(origin, pilot.aim_point, drive.range);
            ghost_tf.translation = dest.extend(0.0);
            ghost_tf.rotation = Quat::from_rotation_z(heading.0);
            *visibility = Visibility::Visible;
        }
    } else {
        *visibility = Visibility::Hidden;
    }
}

/// Draw the enemy fire telegraph: a red ring that closes in as a charging
/// broadside nears firing, plus a line along where the volley will go.
fn draw_charge_telegraph(mut gizmos: Gizmos, ships: Query<(&Transform, &Broadside)>) {
    for (transform, bank) in &ships {
        if bank.charging <= 0.0 || bank.charge_time <= 0.0 {
            continue;
        }
        let pos = transform.translation.truncate();
        // 1 at the start of the wind-up, 0 the instant it fires.
        let t = (bank.charging / bank.charge_time).clamp(0.0, 1.0);
        let radius = 20.0 + t * 70.0;
        let color = Color::srgba(1.0, 0.3, 0.25, 0.85);
        gizmos.circle(Isometry3d::from_translation(pos.extend(3.0)), radius, color);
        let end = pos + bank.charge_dir.normalize_or_zero() * 130.0;
        gizmos.line(pos.extend(3.0), end.extend(3.0), color);
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
    aiming: Res<Aiming>,
    pilot: Res<PilotIntent>,
    mut battery: ResMut<AimBattery>,
) {
    let dt = real.delta_secs();
    let wants_dilation =
        aiming.port || aiming.starboard || pilot.torpedo_hold || pilot.microwarp_hold;
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
    torps: Query<&TorpedoBay, With<Player>>,
    mut hud: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut text) = hud.single_mut() else {
        return;
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
    text.0 = match encounter.outcome {
        Outcome::InProgress => format!(
            "Wave {}  ·  enemies: {}  ·  plundered: {}  ·  {torp_line}\n[LMB/RMB] broadside  [Q] EMP  [Ctrl] torpedoes  [Space] boost  [C] brace  [B] board",
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
