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
    agility_at, Battery, Boarding, Broadside, Encounter, FireBarrelRack, Hull, MicrowarpDrive,
    Outcome, Plunder, Shield, ShieldArc, ShipStats, SimTuning, TorpedoBay, TorpedoLock, Velocity,
};

use crate::bullet_time::AimBattery;
use crate::camera::camera_orbit;
use crate::data::{paths, FeelTuning};
use crate::input::{ControlsPanel, InputMethod, Paused, PlayerAi, ThrustState};
use crate::interpolate::SmoothingSet;
use crate::session::GameState;
use crate::status_ring::{gather_ring_state, RingSnapshot};
use crate::Player;

/// Mounts HUD state-gathering plus the transport systems for this target.
pub struct HudBridgePlugin;

impl Plugin for HudBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudSnapshot>()
            .init_resource::<RingSnapshot>()
            .add_message::<HudAction>()
            // The rings are projected through the camera, so they have to be
            // gathered after the camera has been placed for this frame *and*
            // after the fixed-step poses have been interpolated — otherwise the
            // ring lags its own ship by up to one sim step, which reads as the
            // ring sliding around underneath it.
            .add_systems(
                Update,
                (gather_hud_state, gather_ring_state)
                    .after(SmoothingSet)
                    .after(camera_orbit),
            );

        // Web: mount the iframe overlay and push snapshots into it.
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Startup, web::init).add_systems(
            Update,
            web::push.after(gather_hud_state).after(gather_ring_state),
        );

        // Native desktop: render the HTML HUD with Ultralight into a Bevy texture
        // (opt-in). `init` retries until the window exists, then builds once;
        // `render_hud` pushes state + repaints each frame.
        #[cfg(all(not(target_arch = "wasm32"), feature = "native-html-hud"))]
        {
            info!("HUD: native Ultralight transport ENABLED (native-html-hud feature)");
            app.add_systems(Update, native::init).add_systems(
                Update,
                (
                    // Resize first so a click is mapped against the size the
                    // page is actually laid out at; then forward input, so an
                    // action raised this frame is acted on this frame.
                    native::resize_hud,
                    native::forward_input,
                    native::render_hud,
                )
                    .chain()
                    .after(gather_hud_state)
                    .after(gather_ring_state),
            );
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

/// Which loadout slot a menu chip is talking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Broadside,
    Battery,
    Special,
}

impl Slot {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "broadside" => Some(Self::Broadside),
            "battery" => Some(Self::Battery),
            "special" => Some(Self::Special),
            _ => None,
        }
    }
}

/// An action coming *from* the HUD — a chip on the title card being clicked.
///
/// The page speaks `game.send(action, payload)`; [`parse_hud_action`] turns one
/// drained line into one of these, and `session::apply_hud_actions` is the only
/// thing that acts on them. Keeping the vocabulary small and explicit is what
/// stops the page from being able to ask the game for anything it likes.
#[derive(Message, Debug, Clone, PartialEq)]
pub enum HudAction {
    /// Cast off — begin the run with whatever is currently selected.
    StartRun,
    /// Switch which scenario the next run lays out.
    SelectScenario(&'static str),
    /// Fit `option` into `slot`.
    SelectSlot { slot: Slot, option: usize },
    /// Run it back, from the game-over card.
    Restart,
    /// Something the host does not recognise. Kept rather than dropped so a
    /// mismatch between page and host shows up in a log instead of as silence.
    Unknown(String),
}

/// Parse one drained line — `action|key=value|key=value` — into an action.
///
/// Deliberately hand-rolled and total: the host builds its own JSON with
/// `format!` and carries no parser, and this is the one part of the whole
/// channel that can be tested without a browser or a GPU.
pub fn parse_hud_action(line: &str) -> Option<HudAction> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let mut parts = line.split('|');
    let action = parts.next()?;
    let mut slot = None;
    let mut option = None;
    let mut name = None;
    for pair in parts {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "slot" => slot = Slot::parse(value),
            "option" => option = value.parse::<usize>().ok(),
            "name" => name = Some(value),
            _ => {}
        }
    }

    Some(match action {
        "start_run" => HudAction::StartRun,
        "restart" => HudAction::Restart,
        // The scenario paths are `&'static str` constants, so the page names one
        // rather than handing over a path — a HUD must not be able to ask the
        // game to load an arbitrary file.
        "select_scenario" => match name {
            Some("skirmish") => HudAction::SelectScenario(paths::SKIRMISH),
            Some("test_range") => HudAction::SelectScenario(paths::TEST_RANGE),
            _ => HudAction::Unknown(line.to_string()),
        },
        "select_slot" => match (slot, option) {
            (Some(slot), Some(option)) => HudAction::SelectSlot { slot, option },
            _ => HudAction::Unknown(line.to_string()),
        },
        _ => HudAction::Unknown(line.to_string()),
    })
}

/// Parse a whole drained batch — one action per line.
pub fn parse_hud_actions(text: &str) -> Vec<HudAction> {
    text.lines().filter_map(parse_hud_action).collect()
}

/// The loadout catalogue and the player's standing choice from it, as one system
/// parameter.
///
/// Bundled purely because `gather_hud_state` was already at Bevy's system-param
/// limit. They belong together anyway: neither says anything useful alone.
#[derive(bevy::ecs::system::SystemParam)]
pub struct LoadoutState<'w> {
    catalogue: Res<'w, crate::data::LoadoutCatalogue>,
    fit: Res<'w, crate::data::SelectedLoadout>,
}

/// Read the player ship's live components plus the encounter/session state and
/// rebuild the HUD JSON snapshot.
///
/// Field names and casing here MUST match the `updateHud` schema in `hud.html`:
/// `coords{x,y}`, `hull` (0..1), `boostBattery` (0..1), `portCd`/`starboardCd`/
/// `microwarpCd` as `{remaining,duration}` seconds, `torpedoes[]` of
/// `{state, progress?}`, `torpedoLocks` (u32), `aimBattery` (0..1), `wave`,
/// `enemiesRemaining`, `plunder`, `paused`/`aiPilot` (bool), `boarding`
/// `{active,progress}`, `shield` `{fitted,fore,aft}` (charges as fractions;
/// `fitted` false means the hull carries none, which the HUD must not draw as
/// two broken banks), `sail` `{helm,speed,agility}` (`helm` is a *string-table
/// id* for the notch, which the HUD resolves itself; `speed` and `agility` are
/// fractions of this hull's best), `inputMethod`
/// (`"kbm"`/`"gamepad"`), `outcome`
/// (`"in_progress"`/`"cleared"`/`"destroyed"`), `screen`
/// (`"start"`/`"playing"`/`"gameover"` — which full-screen card, if any, the HUD
/// shows), `controlsOpen` (bool) and `ringOpacity` (0..1, applied to the whole
/// status-ring layer). All weapon components are optional so a
/// partial loadout still reports what it has (the HUD retains last-known values
/// for the rest).
#[allow(clippy::too_many_arguments)]
fn gather_hud_state(
    player: Query<
        (
            &Transform,
            &Hull,
            &Velocity,
            &ShipStats,
            Option<&Shield>,
            Option<&Battery>,
            Option<&Broadside>,
            Option<&MicrowarpDrive>,
            Option<&TorpedoBay>,
            Option<&TorpedoLock>,
            Option<&FireBarrelRack>,
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
    feel: Res<FeelTuning>,
    thrust: Res<ThrustState>,
    tuning: Res<SimTuning>,
    state: Res<State<GameState>>,
    career: Res<crate::progress::Career>,
    loadout: LoadoutState,
    mut snap: ResMut<HudSnapshot>,
) {
    let screen = match state.get() {
        // Loading shows the title card too: it is a handful of frames while the
        // scenario arrives, and flashing a separate screen would read as a bug.
        GameState::Loading | GameState::Menu => "start",
        GameState::Playing => "playing",
        GameState::GameOver => "gameover",
    };

    let mut j = String::with_capacity(384);
    j.push('{');
    j.push_str(&format!("\"screen\":\"{screen}\""));

    // ---- The loadout slots, for the title card ----
    //
    // Emitted on the start screen only: mid-run the card is hidden, and sending
    // a chip list every frame of a fight would be pure noise on the wire. Option
    // names go out as *string ids*, not text — the page resolves them through
    // the same `t()` as everything else it says, so a translation lands in one
    // place rather than in the host's `format!` calls.
    if screen == "start" {
        j.push_str(",\"loadout\":{");
        let slot = |name: &str, chosen: usize, ids: Vec<&str>| {
            let options = ids
                .iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(",");
            format!("\"{name}\":{{\"chosen\":{chosen},\"options\":[{options}]}}")
        };
        j.push_str(&slot(
            "broadside",
            loadout.fit.broadside,
            loadout
                .catalogue
                .broadsides
                .iter()
                .map(|o| o.id.as_str())
                .collect(),
        ));
        j.push(',');
        j.push_str(&slot(
            "battery",
            loadout.fit.battery,
            loadout
                .catalogue
                .batteries
                .iter()
                .map(|o| o.id.as_str())
                .collect(),
        ));
        j.push(',');
        j.push_str(&slot(
            "special",
            loadout.fit.special,
            loadout
                .catalogue
                .specials
                .iter()
                .map(|o| o.id.as_str())
                .collect(),
        ));
        j.push('}');
    }

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

    // The career, for the title card. Emitted every frame like everything else
    // here rather than pushed on change: the snapshot is already a whole-state
    // message, and a second channel for four integers would be two things to
    // keep in step.
    j.push_str(&format!(
        ",\"career\":{{\"runs\":{},\"victories\":{},\"deepestWave\":{},\"plunder\":{}}}",
        career.runs, career.victories, career.deepest_wave, career.ships_boarded
    ));

    j.push_str(&format!(",\"paused\":{}", paused.0));
    j.push_str(&format!(",\"aiPilot\":{}", player_ai.on));

    let boarding_progress = if boarding.target.is_some() {
        (boarding.progress / tuning.board_dwell).clamp(0.0, 1.0)
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

    // Ring opacity rides the snapshot rather than the per-frame ring payload:
    // it is authored, so it changes when a designer drags it and never
    // otherwise, and putting it in the hot channel would mean re-sending a
    // constant sixty times a second.
    j.push_str(&format!(",\"ringOpacity\":{:.3}", feel.rings.opacity));

    // ---- The player ship's own readouts ----
    //
    // Absent before the run starts and after it ends. The HUD retains the last
    // known value for anything missing, so these simply stop updating rather
    // than blanking out.
    if let Ok((tf, hull, vel, stats, shield, battery, broadside, warp, torps, torp_lock, barrels)) =
        player.single()
    {
        // The sim's plane is XY (see `translation.truncate()` uses elsewhere in
        // the client); z is height, ignored by the HUD.
        let (x, y) = (
            tf.translation.x.round() as i64,
            tf.translation.y.round() as i64,
        );
        j.push_str(&format!(",\"coords\":{{\"x\":{x},\"y\":{y}}}"));

        let hull_frac = (hull.current / hull.max).clamp(0.0, 1.0);
        j.push_str(&format!(",\"hull\":{hull_frac:.4}"));

        // Shields. `fitted` is emitted separately from the charges because "no
        // shields on this hull" and "both banks flat" look identical as numbers
        // and must not look identical on screen.
        let fitted = shield.is_some_and(Shield::fitted);
        let (fore, aft) = match shield.filter(|s| s.fitted()) {
            Some(s) => (s.fraction(ShieldArc::Fore), s.fraction(ShieldArc::Aft)),
            None => (0.0, 0.0),
        };
        j.push_str(&format!(
            ",\"shield\":{{\"fitted\":{fitted},\"fore\":{fore:.4},\"aft\":{aft:.4}}}"
        ));

        // Thrust setting and what it is costing. `helm` is the notch the keyboard
        // ladder is on; `speed` and `agility` are what the ship is actually
        // doing, so a pad player (who never touches the ladder) still sees the
        // speed-for-turn-rate bargain they are making. `agility` is normalised
        // against the best the hull can manage, so 1.0 reads as "as handy as
        // this ship gets".
        let speed = vel.0.length();
        let speed_frac = if stats.max_speed > 0.0 {
            (speed / stats.max_speed).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let agility_frac = if stats.turn_rate_slow > 0.0 {
            (agility_at(stats, speed) / stats.turn_rate_slow).clamp(0.0, 1.0)
        } else {
            0.0
        };
        j.push_str(&format!(
            ",\"sail\":{{\"helm\":\"{}\",\"speed\":{speed_frac:.4},\"agility\":{agility_frac:.4}}}",
            thrust.string_key()
        ));

        // One gauge for the whole battery slot, whatever device is drawing on
        // it — the pilot reads charge, not which thing is spending it.
        if let Some(b) = battery {
            j.push_str(&format!(",\"boostBattery\":{:.4}", b.fraction()));
        }

        if let Some(b) = broadside {
            // Per-side reload timer counts down to 0 (ready); cooldown is the full duration.
            push_cooldown(&mut j, "portCd", b.port.timer, b.cooldown);
            push_cooldown(&mut j, "starboardCd", b.starboard.timer, b.cooldown);
        }

        // Which special is fitted. The page *retains* whatever the host stops
        // sending, so without this a microwarp fit would leave a stale six-tube
        // row glowing for the whole run. Sending the discriminator every frame
        // is what lets the page hide the cluster rather than freeze it.
        let special = if torps.is_some() {
            "torpedoes"
        } else if warp.is_some() {
            "microwarp"
        } else if barrels.is_some() {
            "barrels"
        } else {
            "none"
        };
        j.push_str(&format!(",\"special\":\"{special}\""));

        if let Some(w) = warp {
            push_cooldown(&mut j, "microwarpCd", w.timer, w.cooldown);
        }

        if let Some(r) = barrels {
            j.push_str(&format!(
                ",\"barrelMagazine\":{{\"count\":{},\"max\":{}}}",
                r.magazine, r.magazine_max
            ));
        }

        if let Some(t) = torps {
            // Stores, separate from the tubes: the tube row says what can fire
            // now, this says how much fighting is left in the ship.
            j.push_str(&format!(
                ",\"torpedoMagazine\":{{\"count\":{},\"max\":{}}}",
                t.magazine, t.magazine_max
            ));
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

        /// Hand the string table to the overlay. The page also fetches it, so
        /// this is the belt to that braces: a page served without its assets
        /// still gets its text rather than falling back to authored English.
        #[wasm_bindgen(js_namespace = window, js_name = vtStrings)]
        fn vt_strings(json: &str);

        /// Push the per-frame ring transforms. Separate from the snapshot
        /// because it changes on almost every frame while the snapshot does not.
        #[wasm_bindgen(js_namespace = window, js_name = vtHudRings)]
        fn vt_hud_rings(json: &str);

        /// Drain whatever the page's controls have asked for since the last
        /// frame. The overlay is real DOM here, so the clicks arrive on their
        /// own; this is only the collection.
        #[wasm_bindgen(js_namespace = window, js_name = vtHudDrainActions)]
        fn vt_hud_drain_actions() -> String;
    }

    /// Mount the overlay once, at startup, and hand it the strings.
    pub fn init() {
        vt_hud_init();
        vt_strings(crate::strings::TABLE_JSON);
    }

    /// Push the latest snapshot and ring transforms, each only when it changed.
    pub fn push(
        snap: Res<HudSnapshot>,
        rings: Res<super::RingSnapshot>,
        mut actions: MessageWriter<super::HudAction>,
        mut last: Local<u64>,
        mut last_rings: Local<u64>,
    ) {
        if snap.seq != *last {
            *last = snap.seq;
            vt_hud_apply(&snap.json);
        }
        if rings.seq != *last_rings {
            *last_rings = rings.seq;
            vt_hud_rings(&rings.json);
        }
        // Same drain as the native transport, and the same parser — the two
        // hosts differ in how a click *reaches* the page, not in what it says.
        for action in super::parse_hud_actions(&vt_hud_drain_actions()) {
            if let super::HudAction::Unknown(raw) = &action {
                warn!("HUD asked for something this build does not know: {raw}");
            }
            actions.write(action);
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
    use std::sync::Arc;
    use ul_next::{
        config::Config,
        event::{MouseButton as UlMouseButton, MouseEvent, MouseEventType},
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
        /// Kept because `MouseEvent::new` needs it and `Library::linked` returns
        /// a cheap `Arc` clone — there is no reason to reach for it again.
        lib: Arc<Library>,
        /// Last cursor position forwarded, so a still mouse costs no events.
        last_cursor: Option<Vec2>,
        last_seq: u64,
        /// Sequence of the last ring payload evaluated, so an unmoving scene
        /// skips the script entirely.
        last_ring_seq: u64,
    }

    /// Set once init has hard-failed, so it stops retrying.
    #[derive(Resource)]
    struct HudInitFailed;

    /// Marks the fullscreen UI node showing the HUD texture.
    #[derive(Component)]
    struct HudCanvas;

    /// The HUD page with `bridge.js` and the string table inlined, so nothing
    /// has to be resolved from disk.
    ///
    /// The strings matter as much as the bridge here. The page has two ways to
    /// get them — handed over by the host, or fetched from `assets/strings/` —
    /// and on this transport *neither* works on its own: only the wasm shim
    /// implements `vtStrings`, and a document loaded from an in-memory string
    /// has no base URL to resolve a relative `fetch` against. Elements with a
    /// `data-i18n` attribute hide the problem by keeping their authored English,
    /// but every lookup made at runtime — the RDY on a reloaded gun, the whole
    /// controls panel — renders as the missing-string marker.
    ///
    /// Appending the call after the page's own script means the IIFE has already
    /// published `window.__applyStrings` by the time this runs.
    fn hud_document() -> String {
        const HTML: &str = include_str!("../assets/ui/hud.html");
        const BRIDGE: &str = include_str!("../assets/ui/bridge.js");
        let with_bridge = HTML.replace(
            "<script src=\"bridge.js\"></script>",
            &format!("<script>{BRIDGE}</script>"),
        );
        let table = js_string_literal(crate::strings::TABLE_JSON);
        with_bridge.replace(
            "</body>",
            &format!("<script>window.__applyStrings('{table}');</script></body>"),
        )
    }

    /// Escape a string for embedding in a single-quoted JavaScript literal.
    /// Shared by the string table and the per-frame snapshot push so the two
    /// cannot disagree about what needs escaping.
    fn js_string_literal(raw: &str) -> String {
        raw.replace('\\', "\\\\")
            .replace('\'', "\\'")
            // A literal newline inside a JS string literal is a syntax error,
            // and an authored line is perfectly entitled to contain one.
            .replace('\n', "\\n")
            .replace('\r', "\\r")
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
        let Some((w, h, scale)) = world
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .iter(world)
            .next()
            .map(|win| {
                (
                    win.physical_width().max(1) as usize,
                    win.physical_height().max(1) as usize,
                    win.scale_factor(),
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
        // The view is sized in *physical* pixels, so the page's device scale must
        // match or the whole HUD renders at the wrong size — at 150% it was
        // laying out at two-thirds its intended CSS dimensions. Setting it here
        // also makes Bevy's *logical* `cursor_position()` map 1:1 onto page CSS
        // pixels, so forwarding a click needs no conversion in the hot path.
        let Some(view_config) = ViewConfig::start()
            .is_accelerated(false)
            .is_transparent(true)
            .initial_device_scale(scale as f64)
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
        // Ultralight drops input into an unfocused view, so a card full of chips
        // would simply never respond.
        view.focus();

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

        info!("Ultralight HUD initialised ({w}x{h} at {scale}x)");
        world.insert_non_send(HudUl {
            renderer,
            view,
            image: handle,
            width: w,
            height: h,
            lib,
            last_cursor: None,
            last_seq: 0,
            last_ring_seq: 0,
        });
    }

    /// Forward the mouse into the page, and drain whatever it asked for.
    ///
    /// Runs before `render_hud` so a click is seen in the same frame it is made.
    /// Only the *mouse* is forwarded: the HUD has no text fields, and handing
    /// Ultralight the keyboard would fight the game for every key.
    pub fn forward_input(
        hud: Option<NonSendMut<HudUl>>,
        mouse: Res<ButtonInput<MouseButton>>,
        windows: Query<&Window, With<PrimaryWindow>>,
        mut actions: MessageWriter<super::HudAction>,
    ) {
        let Some(mut hud) = hud else {
            return;
        };
        let Ok(window) = windows.single() else {
            return;
        };

        // Logical pixels, which the view's device scale makes equal to page CSS
        // pixels — see `init`.
        if let Some(cursor) = window.cursor_position() {
            let (x, y) = (cursor.x as i32, cursor.y as i32);
            // Ultralight decides what is under the pointer from the *move*, so a
            // press with no preceding move lands on whatever was hovered last.
            // Sending the move first, in the same frame, is what makes a chip
            // clicked on the first frame the mouse arrives register at all.
            if hud.last_cursor != Some(cursor) {
                hud.last_cursor = Some(cursor);
                fire_mouse(&hud, MouseEventType::MouseMoved, x, y, UlMouseButton::None);
            }
            if mouse.just_pressed(MouseButton::Left) {
                fire_mouse(&hud, MouseEventType::MouseDown, x, y, UlMouseButton::Left);
            }
            if mouse.just_released(MouseButton::Left) {
                fire_mouse(&hud, MouseEventType::MouseUp, x, y, UlMouseButton::Left);
            }
        }

        // Ask the page what it wants. An empty queue returns an empty string, so
        // this costs one script evaluation per frame and nothing else.
        let Ok(Ok(drained)) = hud.view.evaluate_script("window.__drainActions()") else {
            return;
        };
        for action in super::parse_hud_actions(&drained) {
            if let super::HudAction::Unknown(raw) = &action {
                warn!("HUD asked for something this build does not know: {raw}");
            }
            actions.write(action);
        }
    }

    /// One mouse event, built and fired. Ultralight frees the event on drop, so
    /// each one is constructed fresh.
    fn fire_mouse(hud: &HudUl, kind: MouseEventType, x: i32, y: i32, button: UlMouseButton) {
        if let Ok(event) = MouseEvent::new(hud.lib.clone(), kind, x, y, button) {
            hud.view.fire_mouse_event(event);
        }
    }

    /// Keep the view, the texture and the click coordinates in step with the
    /// window.
    ///
    /// This is a bug fix as much as a feature: the pixel copy in `render_hud`
    /// walks `hud.width`/`hud.height` against a surface whose stride is re-read
    /// every frame, so a resized window read past the end of the buffer. Nothing
    /// resized before because fullscreen is borderless at desktop resolution —
    /// but a card you can click is a card people will resize the window for.
    pub fn resize_hud(
        hud: Option<NonSendMut<HudUl>>,
        mut resized: MessageReader<bevy::window::WindowResized>,
        windows: Query<&Window, With<PrimaryWindow>>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let Some(mut hud) = hud else {
            resized.clear();
            return;
        };
        if resized.read().last().is_none() {
            return;
        }
        let Ok(window) = windows.single() else {
            return;
        };
        let (w, h) = (
            window.physical_width().max(1) as usize,
            window.physical_height().max(1) as usize,
        );
        if (w, h) == (hud.width, hud.height) {
            return;
        }

        hud.view.resize(w as u32, h as u32);
        hud.width = w;
        hud.height = h;
        if let Some(mut image) = images.get_mut(&hud.image) {
            image.resize(Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: 1,
            });
        }
        // The next frame must repaint into a buffer that just changed shape.
        hud.last_seq = 0;
        hud.last_ring_seq = 0;
    }

    /// Each frame: push the latest snapshot to JS, render Ultralight, and copy the
    /// pixel buffer into the Bevy texture (only when the page repainted).
    pub fn render_hud(
        hud: Option<NonSendMut<HudUl>>,
        snap: Res<HudSnapshot>,
        rings: Res<super::RingSnapshot>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let Some(mut hud) = hud else {
            return;
        };

        hud.renderer.update();

        if snap.seq != hud.last_seq {
            hud.last_seq = snap.seq;
            let esc = js_string_literal(&snap.json);
            let _ = hud
                .view
                .evaluate_script(&format!("window.__applyHud('{esc}')"));
        }

        // Skipped entirely on a frame where nothing moved. That matters more
        // than it looks: the page only repaints when something writes to the
        // DOM, and the pixel copy at the bottom of this function is gated on
        // that repaint — so a still scene costs nothing all the way down.
        if rings.seq != hud.last_ring_seq {
            hud.last_ring_seq = rings.seq;
            let esc = js_string_literal(&rings.json);
            let _ = hud
                .view
                .evaluate_script(&format!("window.__applyRings('{esc}')"));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole HUD->host vocabulary, exercised without a browser. This is the
    /// only part of the channel that can be tested headlessly, which is exactly
    /// why the wire format is a line of text rather than JSON.
    ///
    /// The lines below are the literal output of `bridge.js`'s `__drainActions`
    /// for the matching `game.send` calls, so this is the two languages agreeing
    /// on the format and not just Rust agreeing with itself.
    #[test]
    fn the_page_can_ask_for_the_things_the_host_understands() {
        assert_eq!(parse_hud_action("start_run"), Some(HudAction::StartRun));
        assert_eq!(parse_hud_action("restart"), Some(HudAction::Restart));
        assert_eq!(
            parse_hud_action("select_slot|slot=battery|option=2"),
            Some(HudAction::SelectSlot {
                slot: Slot::Battery,
                option: 2,
            })
        );
        assert_eq!(
            parse_hud_action("select_scenario|name=test_range"),
            Some(HudAction::SelectScenario(paths::TEST_RANGE))
        );
    }

    /// A batch arrives as one string; blank lines are not actions.
    #[test]
    fn a_drained_batch_is_one_action_per_line() {
        let actions = parse_hud_actions("select_slot|slot=special|option=1\n\nstart_run\n");
        assert_eq!(
            actions,
            vec![
                HudAction::SelectSlot {
                    slot: Slot::Special,
                    option: 1,
                },
                HudAction::StartRun,
            ]
        );
    }

    /// Anything the host does not recognise is *kept*, not dropped. A page and a
    /// host that have drifted apart should show up in a log rather than as a
    /// button that silently does nothing.
    #[test]
    fn an_unknown_action_is_reported_rather_than_swallowed() {
        assert!(matches!(
            parse_hud_action("undock|bay=3"),
            Some(HudAction::Unknown(_))
        ));
        // Well-formed name, missing payload — still not something to act on.
        assert!(matches!(
            parse_hud_action("select_slot|slot=battery"),
            Some(HudAction::Unknown(_))
        ));
        // An unknown scenario must never become a path.
        assert!(matches!(
            parse_hud_action("select_scenario|name=../../etc/passwd"),
            Some(HudAction::Unknown(_))
        ));
    }

    #[test]
    fn nothing_at_all_is_not_an_action() {
        assert_eq!(parse_hud_action(""), None);
        assert_eq!(parse_hud_action("   "), None);
        assert!(parse_hud_actions("").is_empty());
    }
}
