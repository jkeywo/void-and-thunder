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
//! Controls:
//!   W / S   — throttle forward / reverse
//!   A / D   — turn to port / starboard
//!   Q / E   — **hold** to aim the port / starboard broadside, **release** to fire
//!   Space   — brace (cut incoming damage)
//!   B       — board a crippled enemy alongside (loot it)
//!   R       — restart after a run ends
//!
//! Gamepad (Black-Flag scheme): RT/LT throttle & reverse, left stick steer,
//! LB/RB hold-aim/release-fire the broadsides, X brace, A board, Start restart.

use bevy::prelude::*;
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
}

/// Which broadsides the player is currently *holding* (aiming). Purely
/// presentational — it drives the aim beam; firing happens on release.
#[derive(Resource, Default)]
struct Aiming {
    port: bool,
    starboard: bool,
}

/// Camera orbit + screen-shake state. `yaw` chases the ship's heading; `trauma`
/// decays each frame and is added to by hits and explosions.
#[derive(Resource, Default)]
struct CameraRig {
    target: Vec2,
    yaw: f32,
    trauma: f32,
    seed: u32,
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
        .add_systems(Startup, setup)
        // Presentation runs in every state.
        .add_systems(
            Update,
            (
                attach_ship_visuals,
                attach_projectile_visuals,
                damage_tint,
                camera_orbit,
                draw_grid,
                draw_aim_beams,
                update_hud,
                update_hull_bar,
            ),
        )
        // Juice: muzzle flashes, hit sparks, explosions, screen shake.
        .add_systems(
            Update,
            (
                muzzle_flashes,
                spawn_hit_effects,
                spawn_destroy_effects,
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
        ship_bundle(Faction::Corsairs, ShipStats::default(), 100.0, PLAYER_START),
        Player,
        Protagonist,
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
fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut board: ResMut<BoardIntent>,
    mut aiming: ResMut<Aiming>,
    mut player: Query<(&mut Helm, &mut FireOrders, &mut Brace), With<Player>>,
) {
    let Ok((mut helm, mut orders, mut brace)) = player.single_mut() else {
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

    let mut aim_port = keys.pressed(KeyCode::KeyQ);
    let mut aim_starboard = keys.pressed(KeyCode::KeyE);
    let mut fire_port = keys.just_released(KeyCode::KeyQ);
    let mut fire_starboard = keys.just_released(KeyCode::KeyE);
    let mut bracing = keys.pressed(KeyCode::Space);
    let mut board_now = keys.just_pressed(KeyCode::KeyB);

    // --- Gamepad (first connected pad), Black-Flag scheme ---
    if let Some(pad) = gamepads.iter().next() {
        let rt = pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0);
        let lt = pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0);
        throttle += rt - lt;

        let stick_x = pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        if stick_x.abs() > STICK_DEADZONE {
            // Stick right (+X) steers starboard (negative turn).
            turn -= stick_x;
        }

        aim_port |= pad.pressed(GamepadButton::LeftTrigger); // LB
        aim_starboard |= pad.pressed(GamepadButton::RightTrigger); // RB
        fire_port |= pad.just_released(GamepadButton::LeftTrigger);
        fire_starboard |= pad.just_released(GamepadButton::RightTrigger);
        bracing |= pad.pressed(GamepadButton::West); // X / Square
        board_now |= pad.just_pressed(GamepadButton::South); // A / Cross
    }

    helm.throttle = throttle.clamp(-1.0, 1.0);
    helm.turn = turn.clamp(-1.0, 1.0);
    aiming.port = aim_port;
    aiming.starboard = aim_starboard;
    // Only ever raise the request — the sim clears it once consumed, so a
    // release is never lost between fixed steps.
    if fire_port {
        orders.port = true;
    }
    if fire_starboard {
        orders.starboard = true;
    }
    brace.active = bracing;
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

/// Give every cannonball a small glowing sphere.
fn attach_projectile_visuals(
    mut commands: Commands,
    meshes: Res<GameMeshes>,
    mats: Res<GameMaterials>,
    shots: Query<Entity, (With<Projectile>, Without<Mesh3d>)>,
) {
    for entity in &shots {
        commands.entity(entity).insert((
            Mesh3d(meshes.sphere.clone()),
            MeshMaterial3d(mats.shot.clone()),
        ));
    }
}

/// Tint each ship's material: darker as its hull wears down, grey when crippled
/// (boardable), and a blue cast while bracing.
fn damage_tint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    ships: Query<
        (
            &Faction,
            &Hull,
            &MeshMaterial3d<StandardMaterial>,
            Option<&Disabled>,
            Option<&Brace>,
        ),
        With<Ship>,
    >,
) {
    for (faction, hull, material, disabled, brace) in &ships {
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
        material.base_color = Color::srgb(r, g, b);
    }
}

/// Orbit the camera around the player: a shallow view from slightly above the
/// plane, yawing to follow the ship's heading, plus screen shake.
fn camera_orbit(
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
    player: Query<(&Transform, &Heading), (With<Player>, Without<MainCamera>)>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta_secs();
    if let Ok((transform, heading)) = player.single() {
        rig.target = transform.translation.truncate();
        // Ease the camera yaw toward the ship's heading so turns swing the view.
        let error = wrap_angle(heading.0 - rig.yaw);
        rig.yaw = wrap_angle(rig.yaw + error * (1.0 - (-CAM_YAW_LERP * dt).exp()));
    }

    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - dt * 1.4).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let shake = Vec3::new(rig.noise(), rig.noise(), rig.noise() * 0.5) * 26.0 * amount;

    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    let forward = Vec2::from_angle(rig.yaw);
    let focus = rig.target.extend(0.0);
    let eye = focus - (forward * CAM_DISTANCE).extend(-CAM_HEIGHT) + shake;
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
    player: Query<(&Transform, &Heading, &Velocity, &Broadside), With<Player>>,
) {
    let Ok((transform, heading, velocity, bank)) = player.single() else {
        return;
    };
    let pos = transform.translation.truncate();
    let color = Color::srgba(1.0, 0.85, 0.4, 0.5);

    for (active, is_port) in [(aiming.port, true), (aiming.starboard, false)] {
        if !active {
            continue;
        }
        for shot in broadside_volley(pos, velocity.0, heading.0, bank, is_port) {
            let dir = shot.velocity.normalize_or_zero();
            let start = shot.position.extend(0.0);
            let end = (shot.position + dir * AIM_BEAM_LEN).extend(0.0);
            gizmos.line(start, end, color);
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
    mut hud: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    text.0 = match encounter.outcome {
        Outcome::InProgress => format!(
            "Wave {}  ·  enemies: {}  ·  plundered: {}\n[W/S] throttle  [A/D] steer  [Q/E] hold to aim, release to fire  [Space] brace  [B] board",
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
