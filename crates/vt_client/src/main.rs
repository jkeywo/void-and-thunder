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

fn setup(mut commands: Commands) {
    commands.spawn((Camera2d, MainCamera));

    // The player's corsair sloop.
    commands.spawn((
        ship(Faction::Corsairs, ShipStats::default(), Vec2::ZERO),
        Player,
    ));

    // A few stationary House hulks to practise broadsides on. Enemy AI and wave
    // spawning are the next MVP step — see docs/mvp-plan.md.
    let heavy = ShipStats {
        thrust: 120.0,
        turn_rate: 1.0,
        max_speed: 160.0,
        ..default()
    };
    for pos in [
        Vec2::new(320.0, 180.0),
        Vec2::new(-360.0, 120.0),
        Vec2::new(60.0, -320.0),
    ] {
        commands.spawn(ship(Faction::Houses, heavy, pos));
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
