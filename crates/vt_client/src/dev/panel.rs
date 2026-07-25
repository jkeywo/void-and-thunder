//! The design panel window.
//!
//! Two ways to edit, deliberately distinguished:
//!
//! * **A ship class** — the authored definition. Edits reach every ship of that
//!   class immediately, survive a restart, and can be saved to disk. This is the
//!   normal way to tune, because "the corsair sloop turns too slowly" is a
//!   statement about the class, not about one hull.
//! * **The selected entity** — a live override on one ship. Useful for a quick
//!   experiment, but it dies with the ship and cannot be saved, so the tab says
//!   so rather than letting you discover it after twenty minutes of work.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use vt_sim::prelude::{
    Broadside, ClassId, EmpWeapon, Hull, MicrowarpDrive, ShipStats, SimTuning, TorpedoBay,
};

use crate::data::{paths, ActiveScenario, DataHandles, FeelTuning, SelectedScenario, ShipTable};
use crate::session::GameState;
use crate::Player;

use super::save::{save_feel, save_ships, save_sim_tuning};
use super::walker::edit_value;
use super::DevPanelFocus;

/// Which target the panel is editing.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Tab {
    #[default]
    Class,
    Entity,
    Sim,
    Feel,
}

/// The panel's own UI state.
#[derive(Resource, Default)]
pub struct DevPanel {
    pub open: bool,
    tab: Tab,
    class: usize,
    /// Set when the class table has been edited but not yet written to disk.
    pub dirty_ships: bool,
    /// Set when the sim tuning has been edited but not yet written to disk.
    pub dirty_tuning: bool,
    /// Set when the feel tuning has been edited but not yet written to disk.
    pub dirty_feel: bool,
    /// Last save result, shown in the panel. Failing silently would look
    /// identical to succeeding, which is the worst outcome for a Save button.
    status: Option<String>,
    status_ok: bool,
}

impl DevPanel {
    fn set_status(&mut self, ok: bool, message: String) {
        self.status_ok = ok;
        self.status = Some(message);
    }

    fn show_status(&self, ui: &mut egui::Ui) {
        let Some(message) = &self.status else {
            return;
        };
        let colour = if self.status_ok {
            egui::Color32::from_rgb(120, 220, 150)
        } else {
            egui::Color32::from_rgb(255, 110, 110)
        };
        ui.colored_label(colour, message);
    }
}

/// `F1` shows and hides the panel.
///
/// Read from `Time<Real>`-driven `Update` rather than the sim schedule, so the
/// panel can still be opened while the game is paused — which is when you most
/// want to be nudging numbers.
pub fn toggle_panel(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<DevPanel>) {
    if keys.just_pressed(KeyCode::F1) {
        panel.open = !panel.open;
    }
}

/// Record whether egui wants the pointer or keyboard this frame.
///
/// Without this, dragging a slider also sweeps the broadside arc and fires on
/// release, because the game's input systems read the raw mouse regardless of
/// what is on top of it.
pub fn track_focus(mut contexts: EguiContexts, mut focus: ResMut<DevPanelFocus>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // `egui_wants_*` (rather than `wants_*`) is egui 0.35's naming: it asks
    // whether *egui* wants the input, which is exactly the question here.
    focus.pointer = ctx.egui_wants_pointer_input() || ctx.is_pointer_over_egui();
    focus.keyboard = ctx.egui_wants_keyboard_input();
}

/// Draw the panel.
#[allow(clippy::too_many_arguments)]
pub fn draw_panel(
    mut contexts: EguiContexts,
    mut panel: ResMut<DevPanel>,
    mut table: ResMut<ShipTable>,
    mut tuning: ResMut<SimTuning>,
    mut feel: ResMut<FeelTuning>,
    // For Revert: the last copy read from disk, which is what "revert" means.
    ship_assets: Res<Assets<ShipTable>>,
    handles: Res<DataHandles>,
    active: Res<ActiveScenario>,
    mut selected: ResMut<SelectedScenario>,
    mut next_state: ResMut<NextState<GameState>>,
    mut player: Query<
        (
            Option<&ClassId>,
            &mut ShipStats,
            &mut Hull,
            &mut Broadside,
            &mut EmpWeapon,
            &mut TorpedoBay,
            &mut MicrowarpDrive,
        ),
        With<Player>,
    >,
) {
    if !panel.open {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::Window::new("Design")
        .default_width(340.0)
        .default_pos([12.0, 12.0])
        .vscroll(true)
        .show(ctx, |ui| {
            scenario_row(ui, &active, &mut selected, &mut next_state);
            ui.separator();

            ui.horizontal(|ui| {
                ui.selectable_value(&mut panel.tab, Tab::Class, "Ship class");
                ui.selectable_value(&mut panel.tab, Tab::Entity, "This ship");
                ui.selectable_value(&mut panel.tab, Tab::Sim, "Sim rules");
                ui.selectable_value(&mut panel.tab, Tab::Feel, "Feel");
            });
            ui.separator();

            match panel.tab {
                Tab::Class => {
                    class_tab(ui, &mut panel, &mut table, ship_assets.get(&handles.ships))
                }
                Tab::Entity => entity_tab(ui, &mut player),
                Tab::Sim => sim_tab(ui, &mut panel, &mut tuning),
                Tab::Feel => feel_tab(ui, &mut panel, &mut feel),
            }
        });
}

/// Scenario picker: which encounter is laid out, and a way to re-lay it.
fn scenario_row(
    ui: &mut egui::Ui,
    active: &ActiveScenario,
    selected: &mut SelectedScenario,
    next_state: &mut NextState<GameState>,
) {
    ui.horizontal(|ui| {
        ui.label("Scenario:");
        egui::ComboBox::from_id_salt("scenario")
            .selected_text(active.0.name.clone())
            .show_ui(ui, |ui| {
                for (label, path) in paths::SCENARIOS {
                    if ui.selectable_label(selected.0 == *path, *label).clicked() {
                        selected.0 = path;
                        // Re-entering Loading clears the field and lays the new
                        // scenario — the same path the title card's [T] uses.
                        next_state.set(GameState::Loading);
                    }
                }
            });
        if ui.button("Reload").clicked() {
            next_state.set(GameState::Loading);
        }
    });
}

/// Edit an authored ship class. Changes reach every ship of that class the same
/// frame, via `apply_class_edits`.
fn class_tab(
    ui: &mut egui::Ui,
    panel: &mut DevPanel,
    table: &mut ResMut<ShipTable>,
    on_disk: Option<&ShipTable>,
) {
    if table.classes.is_empty() {
        ui.label("No ship classes loaded.");
        return;
    }
    panel.class = panel.class.min(table.classes.len() - 1);

    ui.horizontal(|ui| {
        ui.label("Class:");
        let current = table.classes[panel.class].name.clone();
        egui::ComboBox::from_id_salt("class")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, entry) in table.classes.iter().enumerate() {
                    ui.selectable_value(&mut panel.class, i, entry.name.clone());
                }
            });
        if panel.dirty_ships {
            ui.colored_label(egui::Color32::from_rgb(255, 178, 0), "*unsaved");
        }
    });

    ui.horizontal(|ui| {
        if ui.button("Save to ships.ron").clicked() {
            match save_ships(table) {
                Ok(()) => {
                    panel.dirty_ships = false;
                    panel.set_status(true, format!("saved {}", paths::SHIPS));
                }
                Err(e) => panel.set_status(false, e),
            }
        }
        // Revert restores the last copy read from disk, which is what makes
        // experimenting safe: you can push a number somewhere silly and get back
        // to a known state without hunting for the original value.
        ui.add_enabled_ui(on_disk.is_some(), |ui| {
            if ui.button("Revert").clicked() {
                if let Some(disk) = on_disk {
                    **table = disk.clone();
                    panel.dirty_ships = false;
                    panel.set_status(true, "reverted to the file on disk".into());
                }
            }
        });
    });
    panel.show_status(ui);
    ui.separator();

    let index = panel.class;
    // `set_changed` is deliberately *not* called unless something moved: the
    // re-apply pass keys off change detection, and marking the table dirty every
    // frame would stamp the class onto every ship continuously.
    let mut changed = false;
    {
        let class = &mut table.bypass_change_detection().classes[index].class;
        changed |= edit_value(ui, "stats", &mut class.stats, true);
        changed |= edit_value(ui, "hull", &mut class.hull, true);
        changed |= edit_value(ui, "collider", &mut class.collider, true);
        changed |= edit_value(ui, "emp_defense", &mut class.emp_defense, true);
        changed |= edit_value(ui, "loadout", &mut class.loadout, true);
        changed |= edit_value(ui, "ai", &mut class.ai, true);
    }
    if changed {
        table.set_changed();
        panel.dirty_ships = true;
    }
}

/// Edit the player's own components directly — a live override on one ship.
fn entity_tab(
    ui: &mut egui::Ui,
    player: &mut Query<
        (
            Option<&ClassId>,
            &mut ShipStats,
            &mut Hull,
            &mut Broadside,
            &mut EmpWeapon,
            &mut TorpedoBay,
            &mut MicrowarpDrive,
        ),
        With<Player>,
    >,
) {
    let Ok((class, mut stats, mut hull, mut bank, mut emp, mut bay, mut warp)) =
        player.single_mut()
    else {
        ui.label("No player ship in the world.");
        return;
    };

    ui.colored_label(
        egui::Color32::from_rgb(255, 140, 90),
        "Live override — not saved.",
    );
    ui.weak(
        "Wiped by the next class edit, a restart, or a hot reload. \
         Edit the class instead to keep a change.",
    );
    if class.is_none() {
        ui.weak("(this ship has no class, so class edits will never touch it)");
    }
    ui.separator();

    edit_value(ui, "stats", stats.as_mut(), true);
    edit_value(ui, "hull", hull.as_mut(), true);
    edit_value(ui, "broadside", bank.as_mut(), true);
    edit_value(ui, "emp", emp.as_mut(), true);
    edit_value(ui, "torpedoes", bay.as_mut(), true);
    edit_value(ui, "microwarp", warp.as_mut(), true);
}

/// Edit the whole-sim rules.
fn sim_tab(ui: &mut egui::Ui, panel: &mut DevPanel, tuning: &mut ResMut<SimTuning>) {
    ui.horizontal(|ui| {
        ui.label("Applies to every ship at once.");
        if panel.dirty_tuning {
            ui.colored_label(egui::Color32::from_rgb(255, 178, 0), "*unsaved");
        }
    });
    if ui.button("Save to sim.tuning.ron").clicked() {
        match save_sim_tuning(tuning) {
            Ok(()) => {
                panel.dirty_tuning = false;
                panel.set_status(true, format!("saved {}", paths::SIM_TUNING));
            }
            Err(e) => panel.set_status(false, e),
        }
    }
    panel.show_status(ui);
    ui.separator();

    let mut copy = **tuning;
    if edit_value(ui, "sim", &mut copy, true) {
        **tuning = copy;
        panel.dirty_tuning = true;
    }
}

/// Edit how the game feels — bullet-time, shake, trails, the camera.
fn feel_tab(ui: &mut egui::Ui, panel: &mut DevPanel, feel: &mut ResMut<FeelTuning>) {
    ui.horizontal(|ui| {
        ui.label("Presentation only — changes no rule.");
        if panel.dirty_feel {
            ui.colored_label(egui::Color32::from_rgb(255, 178, 0), "*unsaved");
        }
    });
    if ui.button("Save to feel.tuning.ron").clicked() {
        match save_feel(feel) {
            Ok(()) => {
                panel.dirty_feel = false;
                panel.set_status(true, format!("saved {}", paths::FEEL_TUNING));
            }
            Err(e) => panel.set_status(false, e),
        }
    }
    panel.show_status(ui);
    ui.separator();

    let mut copy = **feel;
    if edit_value(ui, "feel", &mut copy, true) {
        **feel = copy;
        panel.dirty_feel = true;
    }
}

/// System set for the panel's UI, so game systems can be ordered after it.
pub fn panel_systems(app: &mut App) {
    app.init_resource::<DevPanel>()
        .add_systems(Update, toggle_panel)
        .add_systems(EguiPrimaryContextPass, (draw_panel, track_focus).chain());
}
