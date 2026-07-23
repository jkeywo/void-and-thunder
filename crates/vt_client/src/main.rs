//! # Void & Thunder — Client
//!
//! The Bevy front end: window, renderer, camera and input. It owns *no* game
//! rules — those live in [`vt_sim`]. This crate spawns ships (sim entities),
//! translates keyboard input into the sim's [`Helm`]/[`FireOrders`] intent
//! components, draws sim entities as sprites, and mounts [`SimPlugin`].
//!
//! Controls (a Black-Flag-style helm):
//!   W / S   — throttle forward / reverse
//!   A / D   — turn to port / starboard
//!   Q       — fire the port broadside
//!   E       — fire the starboard broadside

use bevy::prelude::*;
use vt_sim::prelude::*;

/// Marker for the entity the local player controls.
#[derive(Component)]
struct Player;

/// Marker for the camera so we can make it chase the player.
#[derive(Component)]
struct MainCamera;

/// A background star that parallax-scrolls. `factor` near 1.0 reads as very
/// distant (barely drifts); lower reads as closer (drifts more against the
/// camera). `base` is its resting world position.
#[derive(Component)]
struct Parallax {
    base: Vec2,
    factor: f32,
}

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Void & Thunder".into(),
                        // Let the canvas fill its parent element on the web.
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
        )
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.05)))
        .add_plugins(SimPlugin)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                player_input,
                attach_ship_sprites,
                attach_projectile_sprites,
                camera_follow,
                starfield_parallax,
            ),
        )
        .run();
}

/// Bundle the components that make an entity a ship in the sim.
fn ship(faction: Faction, stats: ShipStats, position: Vec2) -> impl Bundle {
    (
        Ship,
        faction,
        stats,
        Heading(0.0),
        Velocity::default(),
        Helm::default(),
        FireOrders::default(),
        Broadside::default(),
        Hull::new(100.0),
        Collider::default(),
        Transform::from_translation(position.extend(0.0)),
    )
}

fn setup(
    mut commands: Commands,
    bounds: Res<SystemBounds>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    commands.spawn((Camera2d, MainCamera));

    // The star at the heart of the system, plus a couple of stations as
    // landmarks. Drawn as circles behind the ships (negative z).
    spawn_landmark(
        &mut commands,
        &mut meshes,
        &mut materials,
        Vec2::ZERO,
        120.0,
        Color::srgb(1.0, 0.82, 0.42),
    );
    spawn_landmark(
        &mut commands,
        &mut meshes,
        &mut materials,
        Vec2::new(-700.0, 500.0),
        60.0,
        Color::srgb(0.55, 0.60, 0.72),
    );
    spawn_landmark(
        &mut commands,
        &mut meshes,
        &mut materials,
        Vec2::new(820.0, -420.0),
        44.0,
        Color::srgb(0.60, 0.45, 0.40),
    );

    // A parallax starfield backdrop, scattered across the system with a cheap
    // deterministic RNG so it looks the same every run.
    let mut rng = Lcg::new(0x5EED_C0DE);
    let spread = bounds.radius * 2.2;
    for _ in 0..500 {
        let base = Vec2::new((rng.unit() - 0.5) * spread, (rng.unit() - 0.5) * spread);
        let factor = 0.90 + rng.unit() * 0.08; // distant: 0.90..0.98
        let shade = 0.5 + rng.unit() * 0.5;
        commands.spawn((
            Sprite {
                color: Color::srgb(shade, shade, shade * 1.05),
                custom_size: Some(Vec2::splat(1.0 + rng.unit() * 1.5)),
                ..default()
            },
            Transform::from_translation(base.extend(-10.0)),
            Parallax { base, factor },
        ));
    }

    // The player's corsair sloop, offset from the star.
    commands.spawn((
        ship(
            Faction::Corsairs,
            ShipStats::default(),
            Vec2::new(0.0, -520.0),
        ),
        Player,
    ));

    // A few House warships that hunt the player: they pursue, present a
    // broadside and fire (vt_sim::ai). Wave spawning is the next MVP step —
    // see docs/mvp-plan.md.
    let heavy = ShipStats {
        thrust: 120.0,
        turn_rate: 1.4,
        max_speed: 200.0,
        ..default()
    };
    for pos in [
        Vec2::new(320.0, 180.0),
        Vec2::new(-360.0, 120.0),
        Vec2::new(60.0, -320.0),
    ] {
        commands.spawn((ship(Faction::Houses, heavy, pos), AiController::default()));
    }
}

/// Spawn a circular landmark (star/station/planet) behind the ships.
fn spawn_landmark(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    pos: Vec2,
    radius: f32,
    color: Color,
) {
    commands.spawn((
        Landmark { radius },
        Mesh2d(meshes.add(Circle::new(radius))),
        MeshMaterial2d(materials.add(color)),
        Transform::from_translation(pos.extend(-2.0)),
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

/// Translate the keyboard into the player ship's [`Helm`] and [`FireOrders`].
fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut player: Query<(&mut Helm, &mut FireOrders), With<Player>>,
) {
    let Ok((mut helm, mut orders)) = player.single_mut() else {
        return;
    };

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

    helm.throttle = throttle;
    helm.turn = turn;
    orders.port = keys.pressed(KeyCode::KeyQ);
    orders.starboard = keys.pressed(KeyCode::KeyE);
}

/// Give every ship without one a sprite. The long axis points along the bow.
fn attach_ship_sprites(
    mut commands: Commands,
    ships: Query<(Entity, &Faction), (With<Ship>, Without<Sprite>)>,
) {
    for (entity, faction) in &ships {
        let color = match faction {
            Faction::Corsairs => Color::srgb(0.35, 0.85, 0.55),
            Faction::Houses => Color::srgb(0.85, 0.30, 0.30),
            Faction::Janissariat => Color::srgb(0.85, 0.65, 0.20),
            Faction::Guild => Color::srgb(0.45, 0.60, 0.90),
            Faction::Freebooters => Color::srgb(0.75, 0.45, 0.85),
        };
        commands.entity(entity).insert(Sprite {
            color,
            custom_size: Some(Vec2::new(44.0, 20.0)),
            ..default()
        });
    }
}

/// Give every cannonball a small bright sprite.
fn attach_projectile_sprites(
    mut commands: Commands,
    shots: Query<Entity, (With<Projectile>, Without<Sprite>)>,
) {
    for entity in &shots {
        commands.entity(entity).insert(Sprite {
            color: Color::srgb(1.0, 0.9, 0.4),
            custom_size: Some(Vec2::splat(6.0)),
            ..default()
        });
    }
}

/// Drift the starfield against the camera to fake depth. Distant stars
/// (`factor` near 1) barely move; nearer ones slide more.
fn starfield_parallax(
    camera: Query<&Transform, (With<MainCamera>, Without<Parallax>)>,
    mut stars: Query<(&Parallax, &mut Transform), Without<MainCamera>>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let cam = camera.translation.truncate();
    for (parallax, mut transform) in &mut stars {
        let pos = parallax.base + cam * parallax.factor;
        transform.translation.x = pos.x;
        transform.translation.y = pos.y;
    }
}

/// Keep the camera centred on the player's ship.
fn camera_follow(
    player: Query<&Transform, (With<Player>, Without<MainCamera>)>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let (Ok(player), Ok(mut camera)) = (player.single(), camera.single_mut()) else {
        return;
    };
    camera.translation.x = player.translation.x;
    camera.translation.y = player.translation.y;
}
