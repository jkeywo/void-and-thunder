//! HUD bridge — gathers the player ship's live state each frame and produces the
//! JSON snapshot the HTML HUD consumes via `window.updateHud`.
//!
//! The HUD itself is authored as a self-contained web page
//! ([`assets/ui/hud.html`]) with a stable contract ([`assets/ui/bridge.js`]):
//!   * host → HUD: `window.__applyHud('<json>')` parses and calls `updateHud`.
//!   * HUD → host: `game.send(action, payload)` (no controls on this readout HUD yet).
//!
//! Two halves, deliberately separated:
//!   * [`gather_hud_state`] — reads real ECS components (`With<Player>`) and
//!     writes the JSON into [`HudSnapshot`]. Transport-agnostic, no extra deps.
//!   * a *transport* that pushes [`HudSnapshot`] into the live page. Delivered per
//!     target:
//!       - wasm: `hud.html` mounted as a transparent, click-through iframe over
//!         the canvas (vtHud* shim in `index.html`), driven via wasm-bindgen.
//!       - native + `native-webview-hud` feature: a raw `wry` webview overlay
//!         built off Bevy's winit window handle.
//!       - native without the feature: no-op (the Bevy-UI HUD carries native).

use bevy::prelude::*;
use vt_sim::prelude::{BoostDrive, Broadside, Hull, MicrowarpDrive, TorpedoBay};

use crate::Player;

/// Mounts HUD state-gathering plus the transport systems for this target.
pub struct HudBridgePlugin;

impl Plugin for HudBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudSnapshot>()
            .add_message::<HudAction>()
            .add_systems(Update, gather_hud_state);

        // Web: mount the iframe overlay and push snapshots into it.
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Startup, web::init)
            .add_systems(Update, web::push.after(gather_hud_state));

        // Native desktop webview overlay (opt-in).
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-webview-hud"))]
        app.add_systems(Startup, native::init)
            .add_systems(Update, (native::push, native::resize).after(gather_hud_state));
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

    // Only bump when the snapshot actually changed — the HUD retains last-known
    // values, so identical frames need no transport push.
    if j != snap.json {
        snap.json = j;
        snap.seq = snap.seq.wrapping_add(1);
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

// ============================ web (wasm) transport ============================

/// The game is already in a browser, so `hud.html` is mounted as a transparent,
/// click-through iframe over the canvas (see the vtHud* shim in `index.html`)
/// and driven via wasm-bindgen. No webview crate involved.
#[cfg(target_arch = "wasm32")]
mod web {
    use super::HudSnapshot;
    use bevy::prelude::*;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Create the transparent HUD iframe over the canvas (idempotent).
        #[wasm_bindgen(js_namespace = window, js_name = vtHudInit)]
        fn vt_hud_init();

        /// Hand a JSON snapshot to the iframe's `__applyHud` (no-op until it loads).
        #[wasm_bindgen(js_namespace = window, js_name = vtHudApply)]
        fn vt_hud_apply(json: &str);
    }

    /// Mount the overlay once, at startup.
    pub fn init() {
        vt_hud_init();
    }

    /// Push the latest snapshot when it changes.
    pub fn push(snap: Res<HudSnapshot>, mut last: Local<u64>) {
        if snap.seq != *last {
            *last = snap.seq;
            vt_hud_apply(&snap.json);
        }
    }
}

// ========================= native (raw wry) transport ========================

/// A transparent `wry` webview overlay built as a child of Bevy's winit window.
///
/// NOTE (unverified in CI): `build_as_child` supports Windows, macOS and Linux
/// **X11 only** (not Wayland). Also, a full-window child webview will capture
/// pointer/keyboard events over the whole window — for a game HUD it must be made
/// hit-test-transparent per platform (WS_EX_TRANSPARENT on Windows, input shape
/// on X11, `setIgnoresMouseEvents` on macOS) using the webview's native handle.
/// That platform pass is the remaining work before this is playable; the overlay
/// renders and updates without it.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-webview-hud"))]
mod native {
    use super::HudSnapshot;
    use bevy::prelude::*;
    use bevy::window::{PrimaryWindow, WindowResized};
    use bevy::winit::WinitWindows;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{Rect, WebView, WebViewBuilder};

    /// Owns the overlay webview. `!Send`, so it lives as a NonSend resource and
    /// every system touching it runs on the main thread.
    pub(crate) struct HudWebView(WebView);

    /// The HUD page with `bridge.js` inlined (no external file to resolve when
    /// loaded via `with_html`).
    fn hud_document() -> String {
        const HTML: &str = include_str!("../assets/ui/hud.html");
        const BRIDGE: &str = include_str!("../assets/ui/bridge.js");
        HTML.replace(
            "<script src=\"bridge.js\"></script>",
            &format!("<script>{BRIDGE}</script>"),
        )
    }

    /// Build the child webview from the primary window's handle (exclusive system:
    /// inserting a NonSend resource needs `&mut World`).
    pub fn init(world: &mut World) {
        let Some(entity) = world
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .iter(world)
            .next()
        else {
            return;
        };

        let html = hud_document();
        // Build while borrowing the winit window, then drop the borrow before
        // inserting the resource.
        let webview = {
            let Some(winit_windows) = world.get_non_send::<WinitWindows>() else {
                return;
            };
            let Some(window) = winit_windows.get_window(entity) else {
                return;
            };
            let size = window.inner_size();
            WebViewBuilder::new()
                .with_transparent(true)
                .with_bounds(Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: LogicalSize::new(size.width as f64, size.height as f64).into(),
                })
                .with_html(html)
                // `window` is a WindowWrapper; deref to the winit Window, which
                // implements HasWindowHandle.
                .build_as_child(&**window)
        };
        match webview {
            Ok(wv) => world.insert_non_send(HudWebView(wv)),
            Err(e) => error!("failed to build native HUD webview: {e}"),
        }
    }

    /// Push the latest snapshot when it changes, via `__applyHud`.
    pub fn push(snap: Res<HudSnapshot>, webview: Option<NonSend<HudWebView>>, mut last: Local<u64>) {
        let (Some(webview), true) = (webview, snap.seq != *last) else {
            return;
        };
        *last = snap.seq;
        // JSON here is numbers + fixed keys only, but escape defensively for the
        // single-quoted JS string literal.
        let esc = snap.json.replace('\\', "\\\\").replace('\'', "\\'");
        if let Err(e) = webview.0.evaluate_script(&format!("window.__applyHud('{esc}')")) {
            warn!("HUD evaluate_script failed: {e}");
        }
    }

    /// Keep the overlay sized to the window.
    pub fn resize(
        mut resized: MessageReader<WindowResized>,
        webview: Option<NonSend<HudWebView>>,
    ) {
        let Some(webview) = webview else {
            return;
        };
        for ev in resized.read() {
            let _ = webview.0.set_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(ev.width as f64, ev.height as f64).into(),
            });
        }
    }
}
