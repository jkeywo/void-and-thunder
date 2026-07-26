//! Run lifecycle: the `Menu`/`Playing`/`GameOver` state, watching the encounter
//! for an outcome, and resetting the field for a fresh run.

use bevy::asset::LoadState;
use bevy::prelude::*;
use bevy::time::Virtual;
use vt_sim::prelude::*;

use crate::data::{
    director_for, paths, set_director, spawn_scenario, ActiveScenario, DataHandles, Scenario,
    SelectedScenario, ShipTable,
};
use crate::input::SailState;

/// Where the session is: waiting on data, parked on the title card, mid-run, or
/// resolved.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    /// Waiting for the scenario and class table to load.
    ///
    /// This state exists because the ships are *authored*: nothing can be
    /// spawned until the data describing it has arrived, and asset loads finish
    /// some frames after `Startup`.
    #[default]
    Loading,
    /// The start screen. The ship is already spawned and sitting in the void;
    /// the simulation is frozen until the player casts off.
    Menu,
    Playing,
    GameOver,
}

/// Clear the field on the way into [`GameState::Loading`].
///
/// Entering `Loading` is how the game (re)lays an encounter, so this runs on the
/// first load — where it does nothing — and again whenever the scenario changes.
/// Having one entry point means the test range cannot be laid on top of a
/// half-finished skirmish.
pub fn clear_field(
    mut commands: Commands,
    ships: Query<Entity, With<Ship>>,
    projectiles: Query<Entity, With<Projectile>>,
    mut encounter: ResMut<Encounter>,
    mut plunder: ResMut<Plunder>,
    mut board: ResMut<BoardIntent>,
    mut boarding: ResMut<Boarding>,
    mut sail: ResMut<SailState>,
) {
    for entity in ships.iter().chain(&projectiles) {
        commands.entity(entity).despawn();
    }
    *encounter = Encounter::default();
    *plunder = Plunder::default();
    *board = BoardIntent::default();
    *boarding = Boarding::default();
    // The new ship is a new ship: it starts at half sail, not at whatever the
    // last one happened to be set to when it died.
    *sail = SailState::default();
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

/// Wait for the selected scenario to load, then lay out the encounter and show
/// the title card.
///
/// A scenario that fails to load is not fatal: the `Default` scenario and class
/// table reproduce the encounter the game shipped with, so a broken or missing
/// file leaves you playing the normal game rather than staring at a blank void.
pub fn await_data(
    mut commands: Commands,
    server: Res<AssetServer>,
    handles: Res<DataHandles>,
    scenarios: Res<Assets<Scenario>>,
    selected: Res<SelectedScenario>,
    table: Res<ShipTable>,
    mut active: ResMut<ActiveScenario>,
    mut director: ResMut<SpawnDirector>,
    mut bounds: ResMut<SystemBounds>,
    mut next: ResMut<NextState<GameState>>,
) {
    let Some(handle) = handles.scenario(selected.0) else {
        return; // `begin_load` hasn't run yet
    };
    // Both the table and the scenario must have settled, one way or the other.
    let scenario_done = !matches!(server.get_load_state(handle), Some(LoadState::Loading));
    let table_done = !matches!(
        server.get_load_state(&handles.ships),
        Some(LoadState::Loading)
    );
    if !scenario_done || !table_done {
        return;
    }

    let scenario = scenarios.get(handle).cloned().unwrap_or_else(|| {
        warn!(
            "{} did not load — falling back to the built-in encounter",
            selected.0
        );
        Scenario::default()
    });

    enter_scenario(
        &mut commands,
        &table,
        &scenario,
        &mut director,
        &mut bounds,
        &mut active,
    );
    next.set(GameState::Menu);
}

/// Lay out a scenario: set the playfield, spawn its ships, and point the
/// director at its waves (or at nothing).
///
/// Shared by first load and restart so a restarted run cannot drift from a fresh
/// one — the bug being that a restarted test range quietly grows waves.
pub fn enter_scenario(
    commands: &mut Commands,
    table: &ShipTable,
    scenario: &Scenario,
    director: &mut SpawnDirector,
    bounds: &mut SystemBounds,
    active: &mut ActiveScenario,
) {
    bounds.radius = scenario.bounds_radius;
    set_director(director, director_for(scenario, table));
    spawn_scenario(commands, table, scenario);
    active.0 = scenario.clone();
}

/// On the start screen, any of Space / Enter / a click / the pad's South or
/// Start button begins the run — and `T` (or the pad's North) starts it on the
/// test range instead.
///
/// The test range is a keypress rather than a button on the card because the
/// card is a decorative overlay: it is `pointer-events: none`, the native HUD is
/// an Ultralight pixel buffer with no hit-testing, and the HUD→sim action
/// channel is an unused stub. A button would mean building all three.
pub fn start_run(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    gamepads: Query<&Gamepad>,
    mut selected: ResMut<SelectedScenario>,
    mut next: ResMut<NextState<GameState>>,
) {
    let pads: Vec<&Gamepad> = gamepads.iter().collect();
    let pad_test = pads
        .iter()
        .any(|pad| pad.just_pressed(GamepadButton::North));
    if keys.just_pressed(KeyCode::KeyT) || pad_test {
        selected.0 = paths::TEST_RANGE;
        next.set(GameState::Loading); // re-lay the field from the other scenario
        return;
    }

    let pad_start = pads.iter().any(|pad| {
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

/// On the game-over screen, `R` — or the pad's South or Start — clears the field
/// and starts a fresh run.
///
/// South is accepted as well as Start because it is the button that began the
/// run in the first place ([`start_run`]): a pad player who pressed A to cast
/// off will reach for A to sail again, and Start alone left them pressing a
/// button the card never mentioned.
pub fn restart(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut commands: Commands,
    ships: Query<Entity, With<Ship>>,
    projectiles: Query<Entity, With<Projectile>>,
    table: Res<ShipTable>,
    mut active: ResMut<ActiveScenario>,
    mut director: ResMut<SpawnDirector>,
    mut bounds: ResMut<SystemBounds>,
    mut encounter: ResMut<Encounter>,
    mut plunder: ResMut<Plunder>,
    mut board: ResMut<BoardIntent>,
    mut sail: ResMut<SailState>,
    mut next: ResMut<NextState<GameState>>,
) {
    let pad_restart = gamepads.iter().any(|pad| {
        pad.just_pressed(GamepadButton::South) || pad.just_pressed(GamepadButton::Start)
    });
    if !keys.just_pressed(KeyCode::KeyR) && !pad_restart {
        return;
    }
    for entity in ships.iter().chain(&projectiles) {
        commands.entity(entity).despawn();
    }
    reset_encounter(&mut director, &mut encounter);
    *plunder = Plunder::default();
    *board = BoardIntent::default();
    *sail = SailState::default();
    // Re-lay the *same* scenario, through the same path a fresh load uses — a
    // restarted test range must not quietly grow waves.
    let scenario = active.0.clone();
    enter_scenario(
        &mut commands,
        &table,
        &scenario,
        &mut director,
        &mut bounds,
        &mut active,
    );
    next.set(GameState::Playing);
}
