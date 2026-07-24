//! HUD bridge — gathers the player ship's live state each frame and produces the
//! JSON snapshot the HTML HUD consumes via `window.updateHud`.
//!
//! The HUD itself is authored as a self-contained web page
//! ([`assets/ui/hud.html`]) with a stable contract ([`assets/ui/bridge.js`]):
//!   * host → HUD: `window.__applyHud('<json>')` parses and calls `updateHud`.
//!   * HUD → host: `game.send(action, payload)` (no controls on this readout HUD yet).
//!
//! This module is deliberately split into two halves:
//!   * [`gather_hud_state`] — reads real ECS components (`With<Player>`) and
//!     writes the JSON into [`HudSnapshot`]. Renderer/transport agnostic, no
//!     extra dependencies, unit-checkable.
//!   * a *transport* that pushes [`HudSnapshot`] into the actual web page. That
//!     half is platform-specific (a DOM/iframe overlay on wasm; a native webview
//!     otherwise) and is wired separately — see `push_snapshot`.

use bevy::prelude::*;
use vt_sim::prelude::{BoostDrive, Broadside, Hull, MicrowarpDrive, TorpedoBay};

use crate::Player;

/// Mounts the HUD state-gathering. Transport systems are added on top of this
/// per platform (see `push_snapshot`).
pub struct HudBridgePlugin;

impl Plugin for HudBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudSnapshot>()
            .add_message::<HudAction>()
            .add_systems(Startup, init_transport)
            .add_systems(Update, gather_hud_state);
    }
}

/// The most recent HUD state, serialized to the exact JSON shape `updateHud`
/// expects. `seq` bumps whenever the snapshot changes so a transport can skip
/// re-pushing an identical frame.
#[derive(Resource, Default)]
pub struct HudSnapshot {
    pub json: String,
    pub seq: u64,
}

/// An action coming *from* the HUD (a button press). This readout HUD emits none
/// yet; the type exists so the seam is ready when controls are added. A transport
/// parses `game.send(action, payload)` messages into these and dispatches them.
#[derive(Message, Debug, Clone)]
#[allow(dead_code)] // seam: no HUD controls emit actions yet
pub enum HudAction {
    // e.g. FireTorpedo { tube: u8 }, Undock, ... — add as the HUD grows.
    Unknown(String),
}

/// Read the player ship's live components and rebuild the HUD JSON snapshot.
///
/// Field names and casing here MUST match the `updateHud` schema in `hud.html`:
/// `coords{x,y}`, `hull` (0..1), `boostBattery` (0..1), `portCd`/`starboardCd`/
/// `microwarpCd` as `{remaining,duration}` seconds, and `torpedoes[]` of
/// `{state, progress?}`. All weapon components are optional so a partial loadout
/// still reports what it has (the HUD retains last-known values for the rest).
fn gather_hud_state(
    player: Query<
        (
            &Transform,
            &Hull,
            Option<&BoostDrive>,
            Option<&Broadside>,
            Option<&MicrowarpDrive>,
            Option<&TorpedoBay>,
        ),
        With<Player>,
    >,
    mut snap: ResMut<HudSnapshot>,
) {
    let Ok((tf, hull, boost, broadside, warp, torps)) = player.single() else {
        return;
    };

    // The sim's plane is XY (see `translation.truncate()` uses elsewhere in the
    // client); z is height, ignored by the HUD.
    let mut j = String::with_capacity(256);
    j.push('{');

    let (x, y) = (tf.translation.x.round() as i64, tf.translation.y.round() as i64);
    j.push_str(&format!("\"coords\":{{\"x\":{x},\"y\":{y}}}"));

    let hull_frac = (hull.current / hull.max).clamp(0.0, 1.0);
    j.push_str(&format!(",\"hull\":{hull_frac:.4}"));

    if let Some(b) = boost {
        let frac = if b.battery_max > 0.0 {
            (b.battery / b.battery_max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        j.push_str(&format!(",\"boostBattery\":{frac:.4}"));
    }

    if let Some(b) = broadside {
        // Per-side reload timer counts down to 0 (ready); cooldown is the full duration.
        push_cooldown(&mut j, "portCd", b.port.timer, b.cooldown);
        push_cooldown(&mut j, "starboardCd", b.starboard.timer, b.cooldown);
    }

    if let Some(w) = warp {
        push_cooldown(&mut j, "microwarpCd", w.timer, w.cooldown);
    }

    if let Some(t) = torps {
        // `loaded` is fractional: whole part = tubes ready, the fraction is the
        // one tube currently reloading, the rest are empty.
        let ready = t.loaded.floor().max(0.0) as u32;
        let progress = t.loaded.fract().clamp(0.0, 1.0);
        j.push_str(",\"torpedoes\":[");
        for i in 0..t.tubes_max {
            if i > 0 {
                j.push(',');
            }
            if i < ready {
                j.push_str("{\"state\":\"ready\"}");
            } else if i == ready && ready < t.tubes_max {
                j.push_str(&format!("{{\"state\":\"loading\",\"progress\":{progress:.3}}}"));
            } else {
                j.push_str("{\"state\":\"empty\"}");
            }
        }
        j.push(']');
    }

    j.push('}');

    // Only push when the snapshot actually changed — the HUD retains last-known
    // values, so identical frames need no transport call.
    if j != snap.json {
        snap.json = j;
        snap.seq = snap.seq.wrapping_add(1);
        push_snapshot(&snap.json);
    }
}

/// Append a `"name":{"remaining":<s>,"duration":<s>}` cooldown field. `timer` is
/// clamped at 0 so a ready bank reports `remaining: 0`.
fn push_cooldown(j: &mut String, name: &str, timer: f32, duration: f32) {
    j.push_str(&format!(
        ",\"{name}\":{{\"remaining\":{:.3},\"duration\":{:.3}}}",
        timer.max(0.0),
        duration.max(0.0)
    ));
}

// ============================ transport ============================
//
// The HUD page is delivered per target, hidden behind `init_transport` (once at
// startup) and `push_snapshot` (each changed frame):
//   * wasm — the game is already in a browser, so `hud.html` is mounted as a
//     transparent, click-through iframe over the canvas (see the vtHud* shim in
//     index.html) and driven via wasm-bindgen. No webview crate involved.
//   * native — a raw `wry` webview overlay (the `bevy_wry`/`bevy_webview_wry`
//     wrappers don't support Bevy 0.19; raw wry is Bevy-version-independent).
//     Not yet implemented — see the follow-up.

/// Mount the HUD overlay once, at startup.
#[cfg(target_arch = "wasm32")]
fn init_transport() {
    web::vt_hud_init();
}

/// Push the latest JSON snapshot into the live HUD page.
#[cfg(target_arch = "wasm32")]
fn push_snapshot(json: &str) {
    web::vt_hud_apply(json);
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Create the transparent HUD iframe over the canvas (idempotent).
        #[wasm_bindgen(js_namespace = window, js_name = vtHudInit)]
        pub fn vt_hud_init();

        /// Hand a JSON snapshot to the iframe's `__applyHud` (no-op until it loads).
        #[wasm_bindgen(js_namespace = window, js_name = vtHudApply)]
        pub fn vt_hud_apply(json: &str);
    }
}

/// Native transport — a raw `wry` webview overlay. Not yet wired; `HudSnapshot`
/// is fully maintained so this is a drop-in when added.
#[cfg(not(target_arch = "wasm32"))]
fn init_transport() {
    // TODO(native): create a transparent child wry WebView from Bevy's winit
    // window handle, load assets/ui/hud.html, inject bridge.js.
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(unused_variables)]
fn push_snapshot(json: &str) {
    // TODO(native): webview.evaluate_script(&format!("window.__applyHud('{}')", esc));
}
