//! The in-game design panel — a development tool, behind the `dev-panel`
//! feature.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p vt_client --features dev-panel,fast-compile
//! ```
//!
//! then press `F1`. Pick a ship class (or the sim's own rules) and move the
//! numbers with paired sliders and text boxes: the slider to find the shape of a
//! value, the box to type the exact figure. Edits land on the ships already
//! flying, the same frame.
//!
//! ## What is and isn't behind the feature
//!
//! Only the *UI*. Data loading, the tuning resources, ship classes and scenarios
//! are unconditional — gating those would mean a release build silently ran on
//! `Default` values while the tuned RON sat unread on disk.
//!
//! [`DevPanelFocus`] therefore exists in both builds, always `false` when the
//! feature is off, so the input systems can carry one run condition rather than
//! a `cfg` each.

use bevy::prelude::*;

#[cfg(feature = "dev-panel")]
mod apply;
#[cfg(feature = "dev-panel")]
mod fields;
#[cfg(feature = "dev-panel")]
mod panel;
#[cfg(feature = "dev-panel")]
mod save;
#[cfg(feature = "dev-panel")]
mod walker;

/// Whether the design panel is swallowing input this frame.
///
/// egui captures the pointer when it is over a window, and the keyboard while a
/// text box has focus. Without honouring that, dragging a slider across the
/// viewport also sweeps the broadside arc and fires on release — the game reads
/// the raw mouse and has no idea a panel is on top of it.
///
/// Always `false` without the `dev-panel` feature, so the run condition below
/// costs a bool check and nothing else in a normal build.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct DevPanelFocus {
    pub pointer: bool,
    pub keyboard: bool,
}

/// Run condition: true when the *game* should act on input.
///
/// Free-look is gated on the pointer only — a camera that stopped following
/// while you typed in a text box would read as a bug.
pub fn game_has_input(focus: Res<DevPanelFocus>) -> bool {
    !focus.pointer && !focus.keyboard
}

/// Run condition: true when the game should act on the pointer specifically.
pub fn game_has_pointer(focus: Res<DevPanelFocus>) -> bool {
    !focus.pointer
}

/// Mounts the design panel, or nothing at all.
pub struct DevPanelPlugin;

impl Plugin for DevPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DevPanelFocus>();

        #[cfg(feature = "dev-panel")]
        {
            app.add_plugins(bevy_egui::EguiPlugin::default());
            panel::panel_systems(app);
            app.add_systems(Update, apply::apply_class_edits);
            info!("design panel enabled — press F1");
        }
    }
}
