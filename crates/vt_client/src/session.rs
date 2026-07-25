//! Run lifecycle: the `Menu`/`Playing`/`GameOver` state, watching the encounter
//! for an outcome, and resetting the field for a fresh run.

use bevy::prelude::*;
use bevy::time::Virtual;
use vt_sim::prelude::*;

use crate::spawn_player;

/// Where the session is: parked on the title card, mid-run, or resolved.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    /// The start screen. The ship is already spawned and sitting in the void;
    /// the simulation is frozen until the player casts off.
    #[default]
    Menu,
    Playing,
    GameOver,
}

/// Freeze the simulation while the title card is up.
///
/// This is the same lever `toggle_pause` pulls: a paused `Time<Virtual>` stops
/// every `FixedUpdate` system at once, which matters here for more than tidiness
/// — with the director frozen no waves spawn before the player has started, and
/// its "no protagonist means the run is lost" branch can never fire early.
pub fn freeze_for_menu(mut virt: ResMut<Time<Virtual>>) {
    virt.pause();
}

/// Let the simulation run once the player casts off.
pub fn unfreeze_for_run(mut virt: ResMut<Time<Virtual>>) {
    virt.unpause();
}

/// On the start screen, any of Space / Enter / a click / the pad's South or
/// Start button begins the run.
pub fn start_run(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut next: ResMut<NextState<GameState>>,
) {
    let pad_start = gamepads.iter().any(|pad| {
        pad.just_pressed(GamepadButton::South) || pad.just_pressed(GamepadButton::Start)
    });
    let key_start = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::NumpadEnter);
    if key_start || mouse.just_pressed(MouseButton::Left) || pad_start {
        next.set(GameState::Playing);
    }
}

/// Move to the game-over state once the encounter has resolved.
pub fn watch_outcome(encounter: Res<Encounter>, mut next: ResMut<NextState<GameState>>) {
    if encounter.outcome != Outcome::InProgress {
        next.set(GameState::GameOver);
    }
}

/// On the game-over screen, `R` (or the pad's Start) clears the field and starts
/// a fresh run.
pub fn restart(
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
