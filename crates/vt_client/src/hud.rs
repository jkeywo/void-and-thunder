//! HUD bridge — gathers the player ship's live state each frame and produces the
//! JSON snapshot the HTML HUD consumes via `window.updateHud`. This is the
//! canonical HUD: it also carries the encounter status, pause/AI/boarding
//! state, and the controls side panel that the legacy Bevy-Text HUD used to
//! show — see `gather_hud_state`'s doc comment for the full JSON contract.
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
//!       - wasm: `hud.html`'s style + body are injected into a same-document
//!         overlay `<div>` (see the `vtHud*` shim in `index.html` — a
//!         transparent *iframe* over a WebGL canvas composites opaque in
//!         Chromium and hides the game, so this isn't an iframe).
//!       - native + `native-html-hud` feature: Ultralight renders `hud.html`
//!         off-screen into a Bevy texture, composited as a fullscreen UI node.
//!       - native without the feature: no-op (no HUD renders).

use bevy::prelude::*;
use vt_sim::prelude::{
    Boarding, BoostDrive, Broadside, Encounter, Hull, MicrowarpDrive, Outcome, Plunder, TorpedoBay,
    TorpedoLock, BOARD_DWELL,
};

use crate::bullet_time::AimBattery;
use crate::input::{ControlsPanel, InputMethod, Paused, PlayerAi};
use crate::session::GameState;
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

        // Native desktop: render the HTML HUD with Ultralight into a Bevy texture
        // (opt-in). `init` retries until the window exists, then builds once;
        // `render_hud` pushes state + repaints each frame.
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-html-hud"))]
        {
            info!("HUD: native Ultralight transport ENABLED (native-html-hud feature)");
            app.add_systems(Update, native::init)
                .add_systems(Update, native::render_hud.after(gather_hud_state));
        }
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

/// Read the player ship's live components plus the encounter/session state and
/// rebuild the HUD JSON snapshot.
///
/// Field names and casing here MUST match the `updateHud` schema in `hud.html`:
/// `coords{x,y}`, `hull` (0..1), `boostBattery` (0..1), `portCd`/`starboardCd`/
/// `microwarpCd` as `{remaining,duration}` seconds, `torpedoes[]` of
/// `{state, progress?}`, `torpedoLocks` (u32), `aimBattery` (0..1), `wave`,
/// `enemiesRemaining`, `plunder`, `paused`/`aiPilot` (bool), `boarding`
/// `{active,progress}`, `inputMethod` (`"kbm"`/`"gamepad"`), `outcome`
/// (`"in_progress"`/`"cleared"`/`"destroyed"`), `screen`
/// (`"start"`/`"playing"`/`"gameover"` — which full-screen card, if any, the HUD
/// shows), and `controlsOpen` (bool). All weapon components are optional so a
/// partial loadout still reports what it has (the HUD retains last-known values
/// for the rest).
#[allow(clippy::too_many_arguments)]
fn gather_hud_state(
    player: Query<
        (
            &Transform,
            &Hull,
            Option<&BoostDrive>,
            Option<&Broadside>,
            Option<&MicrowarpDrive>,
            Option<&TorpedoBay>,
            Option<&TorpedoLock>,
        ),
        With<Player>,
    >,
    encounter: Res<Encounter>,
    plunder: Res<Plunder>,
    paused: Res<Paused>,
    player_ai: Res<PlayerAi>,
    boarding: Res<Boarding>,
    method: Res<InputMethod>,
    battery: Res<AimBattery>,
    controls_panel: Res<ControlsPanel>,
    state: Res<State<GameState>>,
    mut snap: ResMut<HudSnapshot>,
) {
    let screen = match state.get() {
        GameState::Menu => "start",
        GameState::Playing => "playing",
        GameState::GameOver => "gameover",
    };

    let mut j = String::with_capacity(384);
    j.push('{');
    j.push_str(&format!("\"screen\":\"{screen}\""));

    // ---- Session and encounter state, all from resources ----
    //
    // These are emitted unconditionally, *before* the ship readouts, precisely
    // because the ship may be gone: the player's hull is destroyed in the same
    // sim step that resolves the run, so gating `outcome` on the ship existing
    // would mean the end-of-run banner never got told the run had ended.

    let aim_frac = (battery.charge / battery.max).clamp(0.0, 1.0);
    j.push_str(&format!(",\"aimBattery\":{aim_frac:.4}"));

    j.push_str(&format!(",\"wave\":{}", encounter.wave.max(1)));
    j.push_str(&format!(
        ",\"enemiesRemaining\":{}",
        encounter.enemies_remaining
    ));
    j.push_str(&format!(",\"plunder\":{}", plunder.ships_boarded));

    j.push_str(&format!(",\"paused\":{}", paused.0));
    j.push_str(&format!(",\"aiPilot\":{}", player_ai.on));

    let boarding_progress = if boarding.target.is_some() {
        (boarding.progress / BOARD_DWELL).clamp(0.0, 1.0)
    } else {
        0.0
    };
    j.push_str(&format!(
        ",\"boarding\":{{\"active\":{},\"progress\":{:.3}}}",
        boarding.target.is_some(),
        boarding_progress
    ));

    let input_method = match *method {
        InputMethod::KeyboardMouse => "kbm",
        InputMethod::Gamepad => "gamepad",
    };
    j.push_str(&format!(",\"inputMethod\":\"{input_method}\""));

    let outcome = match encounter.outcome {
        Outcome::InProgress => "in_progress",
        Outcome::Cleared => "cleared",
        Outcome::PlayerDestroyed => "destroyed",
    };
    j.push_str(&format!(",\"outcome\":\"{outcome}\""));

    j.push_str(&format!(",\"controlsOpen\":{}", controls_panel.open));

    // ---- The player ship's own readouts ----
    //
    // Absent before the run starts and after it ends. The HUD retains the last
    // known value for anything missing, so these simply stop updating rather
    // than blanking out.
    if let Ok((tf, hull, boost, broadside, warp, torps, torp_lock)) = player.single() {
        // The sim's plane is XY (see `translation.truncate()` uses elsewhere in
        // the client); z is height, ignored by the HUD.
        let (x, y) = (
            tf.translation.x.round() as i64,
            tf.translation.y.round() as i64,
        );
        j.push_str(&format!(",\"coords\":{{\"x\":{x},\"y\":{y}}}"));

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
            // `loaded` is fractional: whole part = tubes ready, the fraction is
            // the one tube currently reloading, the rest are empty.
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
                    j.push_str(&format!(
                        "{{\"state\":\"loading\",\"progress\":{progress:.3}}}"
                    ));
                } else {
                    j.push_str("{\"state\":\"empty\"}");
                }
            }
            j.push(']');
        }

        let locks = torp_lock.map_or(0, |l| l.locks);
        j.push_str(&format!(",\"torpedoLocks\":{locks}"));
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

// ================== native (Ultralight → Bevy texture) transport ==================

/// Renders the HTML HUD with Ultralight to a CPU pixel buffer, uploads it to a
/// Bevy texture, and draws it as a fullscreen UI node — so Bevy composites the
/// HUD over the game with proper alpha at native performance. No OS-window
/// overlay, no compositing hacks.
///
/// Runtime needs: `ul-next` links the downloaded Ultralight SDK, and Ultralight
/// expects its SDK `resources/` folder in the working directory.
#[cfg(all(not(target_arch = "wasm32"), feature = "native-html-hud"))]
mod native {
    use super::HudSnapshot;
    use bevy::asset::RenderAssetUsages;
    use bevy::image::Image;
    use bevy::prelude::*;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    use bevy::window::PrimaryWindow;
    use ul_next::{
        config::Config,
        platform,
        renderer::Renderer,
        view::{View, ViewConfig},
        Library,
    };

    /// Ultralight renderer + view + the target texture. `!Send`, so it lives as a
    /// NonSend resource and only main-thread systems touch it.
    pub(crate) struct HudUl {
        renderer: Renderer,
        view: View,
        image: Handle<Image>,
        width: usize,
        height: usize,
        last_seq: u64,
    }

    /// Set once init has hard-failed, so it stops retrying.
    #[derive(Resource)]
    struct HudInitFailed;

    /// Marks the fullscreen UI node showing the HUD texture.
    #[derive(Component)]
    struct HudCanvas;

    /// The HUD page with `bridge.js` inlined (no external file to resolve).
    fn hud_document() -> String {
        const HTML: &str = include_str!("../assets/ui/hud.html");
        const BRIDGE: &str = include_str!("../assets/ui/bridge.js");
        HTML.replace(
            "<script src=\"bridge.js\"></script>",
            &format!("<script>{BRIDGE}</script>"),
        )
    }

    /// Create the Ultralight renderer/view + target texture once the window
    /// exists. Exclusive system: it spawns UI and inserts a NonSend resource.
    /// Runs every frame but does its work once (retries while the window is not
    /// ready, then stops on success or a logged hard failure).
    pub fn init(world: &mut World) {
        if world.get_non_send::<HudUl>().is_some()
            || world.get_resource::<HudInitFailed>().is_some()
        {
            return;
        }

        // Size the HUD to the window; wait for it to exist.
        let Some((w, h)) = world
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .iter(world)
            .next()
            .map(|win| {
                (
                    win.physical_width().max(1) as usize,
                    win.physical_height().max(1) as usize,
                )
            })
        else {
            return;
        };

        let fail = |world: &mut World, msg: &str| {
            error!("Ultralight HUD init failed: {msg}");
            world.insert_resource(HudInitFailed);
        };

        // The Ultralight SDK is linked; grab the library handle for the builders
        // and platform setup (fonts + filesystem for the SDK `resources/`).
        let lib = Library::linked();
        platform::enable_platform_fontloader(lib.clone());
        platform::enable_platform_filesystem(lib.clone(), ".").ok();
        platform::enable_default_logger(lib.clone(), "./ultralight.log").ok();

        let Some(config) = Config::start().build(lib.clone()) else {
            return fail(world, "Config::build");
        };
        let Ok(renderer) = Renderer::create(config) else {
            return fail(world, "Renderer::create");
        };
        let Some(view_config) = ViewConfig::start()
            .is_accelerated(false)
            .is_transparent(true)
            .build(lib.clone())
        else {
            return fail(world, "ViewConfig::build");
        };
        let Some(view) = renderer.create_view(w as u32, h as u32, &view_config, None) else {
            return fail(world, "create_view");
        };
        if view.load_html(&hud_document()).is_err() {
            return fail(world, "load_html");
        }

        // Target texture, transparent to start, CPU-writable + rendered.
        let image = Image::new_fill(
            Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 0],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        );
        let handle = world.resource_mut::<Assets<Image>>().add(image);

        // Fullscreen UI node drawing the HUD over the game.
        world.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ImageNode::new(handle.clone()),
            HudCanvas,
        ));

        info!("Ultralight HUD initialised ({w}x{h})");
        world.insert_non_send(HudUl {
            renderer,
            view,
            image: handle,
            width: w,
            height: h,
            last_seq: 0,
        });
    }

    /// Each frame: push the latest snapshot to JS, render Ultralight, and copy the
    /// pixel buffer into the Bevy texture (only when the page repainted).
    pub fn render_hud(
        hud: Option<NonSendMut<HudUl>>,
        snap: Res<HudSnapshot>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let Some(mut hud) = hud else {
            return;
        };

        hud.renderer.update();

        if snap.seq != hud.last_seq {
            hud.last_seq = snap.seq;
            let esc = snap.json.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = hud
                .view
                .evaluate_script(&format!("window.__applyHud('{esc}')"));
        }

        hud.renderer.render();

        // `render()` consumes the view's paint flag, so gate the re-upload on the
        // *surface* dirty bounds instead — non-empty means it repainted this frame.
        let Some(mut surface) = hud.view.surface() else {
            return;
        };
        if surface.dirty_bounds().is_empty() {
            return;
        }
        let row_bytes = surface.row_bytes() as usize;
        let (w, h) = (hud.width, hud.height);

        if let Some(mut image) = images.get_mut(&hud.image) {
            if let Some(dst) = image.data.as_mut() {
                if let Some(pixels) = surface.lock_pixels() {
                    // Ultralight surface is premultiplied BGRA; Bevy UI blends
                    // straight alpha, so un-premultiply into RGBA as we copy.
                    for y in 0..h {
                        let src_row = y * row_bytes;
                        let dst_row = y * w * 4;
                        for x in 0..w {
                            let si = src_row + x * 4;
                            let di = dst_row + x * 4;
                            let (b, g, r, a) =
                                (pixels[si], pixels[si + 1], pixels[si + 2], pixels[si + 3]);
                            if a == 0 {
                                dst[di] = 0;
                                dst[di + 1] = 0;
                                dst[di + 2] = 0;
                                dst[di + 3] = 0;
                            } else {
                                let un = |c: u8| ((c as u16 * 255) / a as u16).min(255) as u8;
                                dst[di] = un(r);
                                dst[di + 1] = un(g);
                                dst[di + 2] = un(b);
                                dst[di + 3] = a;
                            }
                        }
                    }
                }
            }
        }

        surface.clear_dirty_bounds();
    }
}
