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
//!   Q / E   — fire the port / starboard broadside
//!   Space   — brace (cut incoming damage)
//!   B       — board a crippled enemy alongside (loot it)
//!   R       — restart after a run ends
//!
//! Gamepad (Black-Flag scheme): RT/LT throttle & reverse, left stick steer,
//! LB/RB port/starboard broadside, X brace, A board, Start restart.

use bevy::prelude::*;
use vt_sim::prelude::*;

mod audio;
use audio::SfxPlugin;

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

/// Marker for the fill bar of the player's hull gauge.
#[derive(Component)]
struct HullBarFill;

/// Camera follow target plus screen-shake state. `trauma` decays each frame and
/// is added to by hits and explosions; shake offset scales with `trauma²`.
#[derive(Resource, Default)]
struct CameraRig {
    target: Vec2,
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

/// Spawn a scaling, fading effect sprite at a world position.
fn spawn_effect(
    commands: &mut Commands,
    pos: Vec2,
    size: f32,
    start_scale: f32,
    end_scale: f32,
    life: f32,
    color: Color,
) {
    commands.spawn((
        Sprite {
            color,
            custom_size: Some(Vec2::splat(size)),
            ..default()
        },
        Transform::from_translation(pos.extend(1.0)),
        Effect {
            age: 0.0,
            life,
            start_scale,
            end_scale,
            color,
        },
    ));
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
        .add_systems(Startup, setup)
        // Presentation runs in every state.
        .add_systems(
            Update,
            (
                attach_ship_sprites,
                attach_projectile_sprites,
                damage_tint,
                camera_follow,
                starfield_parallax,
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

/// Deadzone below which stick input is ignored.
const STICK_DEADZONE: f32 = 0.15;

/// Translate keyboard **and** gamepad into the player ship's helm, fire orders,
/// brace and boarding intent.
///
/// The pad uses a Black-Flag naval scheme:
///   RT / LT      — throttle forward / reverse (analog)
///   left stick X — steer
///   LB / RB      — fire port / starboard broadside
///   X (West)     — brace
///   A (South)    — board a crippled ship
fn player_input(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut board: ResMut<BoardIntent>,
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

    let mut port = keys.pressed(KeyCode::KeyQ);
    let mut starboard = keys.pressed(KeyCode::KeyE);
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

        port |= pad.pressed(GamepadButton::LeftTrigger); // LB
        starboard |= pad.pressed(GamepadButton::RightTrigger); // RB
        bracing |= pad.pressed(GamepadButton::West); // X / Square
        board_now |= pad.just_pressed(GamepadButton::South); // A / Cross
    }

    helm.throttle = throttle.clamp(-1.0, 1.0);
    helm.turn = turn.clamp(-1.0, 1.0);
    orders.port = port;
    orders.starboard = starboard;
    brace.active = bracing;
    if board_now {
        board.active = true;
    }
}

/// Give every ship without one a sprite. The long axis points along the bow.
fn attach_ship_sprites(
    mut commands: Commands,
    ships: Query<(Entity, &Faction), (With<Ship>, Without<Sprite>)>,
) {
    for (entity, faction) in &ships {
        commands.entity(entity).insert(Sprite {
            color: faction_color(faction),
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

/// Follow the player and apply screen shake. When the player is gone the rig
/// holds its last target, so the death explosion still shakes in place.
fn camera_follow(
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
    player: Query<&Transform, (With<Player>, Without<MainCamera>)>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    if let Ok(player) = player.single() {
        rig.target = player.translation.truncate();
    }
    // Decay trauma; shake amount is trauma squared for a punchy falloff.
    rig.trauma = (rig.trauma - time.delta_secs() * 1.4).clamp(0.0, 1.0);
    let amount = rig.trauma * rig.trauma;
    let offset = Vec2::new(rig.noise(), rig.noise()) * 26.0 * amount;

    let Ok(mut camera) = camera.single_mut() else {
        return;
    };
    camera.translation.x = rig.target.x + offset.x;
    camera.translation.y = rig.target.y + offset.y;
}

/// Tint each ship's sprite: darker as its hull wears down, grey when crippled
/// (boardable), and a blue cast while bracing.
fn damage_tint(
    mut ships: Query<
        (
            &Faction,
            &Hull,
            &mut Sprite,
            Option<&Disabled>,
            Option<&Brace>,
        ),
        With<Ship>,
    >,
) {
    for (faction, hull, mut sprite, disabled, brace) in &mut ships {
        if disabled.is_some() {
            // Crippled hulk — drifting, boardable.
            sprite.color = Color::srgb(0.42, 0.44, 0.5);
            continue;
        }
        let frac = (hull.current / hull.max).clamp(0.0, 1.0);
        let k = 0.4 + 0.6 * frac;
        let base = faction_color(faction).to_srgba();
        let (mut r, mut g, mut b) = (base.red * k, base.green * k, base.blue * k);
        if brace.is_some_and(|brace| brace.active) {
            // Wash toward a cold brace-blue.
            r = r * 0.5;
            g = g * 0.6 + 0.3;
            b = b * 0.5 + 0.5;
        }
        sprite.color = Color::srgb(r, g, b);
    }
}

/// A muzzle flash blooms wherever a new cannonball appears.
fn muzzle_flashes(mut commands: Commands, new_shots: Query<&Transform, Added<Projectile>>) {
    for transform in &new_shots {
        spawn_effect(
            &mut commands,
            transform.translation.truncate(),
            10.0,
            1.4,
            0.2,
            0.12,
            Color::srgb(1.0, 0.95, 0.6),
        );
    }
}

/// Sparks and a little shake when a hull is hit.
fn spawn_hit_effects(
    mut commands: Commands,
    mut hits: MessageReader<ShipHit>,
    mut rig: ResMut<CameraRig>,
) {
    for hit in hits.read() {
        spawn_effect(
            &mut commands,
            hit.position,
            9.0,
            0.6,
            1.8,
            0.22,
            Color::srgb(1.0, 0.7, 0.3),
        );
        rig.add_trauma(0.12);
    }
}

/// An expanding blast and a bigger shake when a ship is destroyed.
fn spawn_destroy_effects(
    mut commands: Commands,
    mut destroyed: MessageReader<ShipDestroyed>,
    mut rig: ResMut<CameraRig>,
) {
    for kill in destroyed.read() {
        spawn_effect(
            &mut commands,
            kill.position,
            26.0,
            0.5,
            3.0,
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
    mut effects: Query<(Entity, &mut Effect, &mut Transform, &mut Sprite)>,
) {
    let dt = time.delta_secs();
    for (entity, mut effect, mut transform, mut sprite) in &mut effects {
        effect.age += dt;
        let t = (effect.age / effect.life).clamp(0.0, 1.0);
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let scale = effect.start_scale + (effect.end_scale - effect.start_scale) * t;
        transform.scale = Vec3::splat(scale);
        sprite.color = effect.color.with_alpha(1.0 - t);
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
            "Wave {}  ·  enemies: {}  ·  plundered: {}\n[W/S] throttle  [A/D] steer  [Q/E] broadside  [Space] brace  [B] board crippled",
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
