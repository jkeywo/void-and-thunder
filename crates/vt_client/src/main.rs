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

/// Marker for the heads-up text.
#[derive(Component)]
struct HudText;

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
        .init_state::<GameState>()
        .add_systems(Startup, setup)
        // Presentation runs in every state.
        .add_systems(
            Update,
            (
                attach_ship_sprites,
                attach_projectile_sprites,
                camera_follow,
                starfield_parallax,
                update_hud,
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

    // The player's corsair sloop. Enemy waves are spawned by the sim's
    // SpawnDirector, not here.
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

/// Keep the camera centred on the player's ship. When the player is gone
/// (destroyed) the camera simply holds its last position.
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

/// Move to the game-over state once the encounter has resolved.
fn watch_outcome(encounter: Res<Encounter>, mut next: ResMut<NextState<GameState>>) {
    if encounter.outcome != Outcome::InProgress {
        next.set(GameState::GameOver);
    }
}

/// On the game-over screen, `R` clears the field and starts a fresh run.
fn restart(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    ships: Query<Entity, With<Ship>>,
    projectiles: Query<Entity, With<Projectile>>,
    mut director: ResMut<SpawnDirector>,
    mut encounter: ResMut<Encounter>,
    mut next: ResMut<NextState<GameState>>,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    for entity in ships.iter().chain(&projectiles) {
        commands.entity(entity).despawn();
    }
    reset_encounter(&mut director, &mut encounter);
    spawn_player(&mut commands);
    next.set(GameState::Playing);
}

/// Update the heads-up text with the wave, enemies left, and any outcome.
fn update_hud(encounter: Res<Encounter>, mut hud: Query<&mut Text, With<HudText>>) {
    let Ok(mut text) = hud.single_mut() else {
        return;
    };
    text.0 = match encounter.outcome {
        Outcome::InProgress => format!(
            "Wave {}  ·  enemies: {}",
            encounter.wave.max(1),
            encounter.enemies_remaining
        ),
        Outcome::Cleared => format!(
            "ALL {} WAVES CLEARED — the lanes are yours.\nPress R to sail again.",
            encounter.wave
        ),
        Outcome::PlayerDestroyed => {
            "YOUR SHIP IS LOST TO THE VOID.\nPress R to sail again.".to_string()
        }
    };
}
