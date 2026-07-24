// bridge.js — the stable seam between the game host and any HUD mockup.
// Injected before the HUD page's own scripts (e.g. as a webview initialization
// script, or a <script src> ahead of hud.html's inline block). Mockups define
// updateHud()/handlers only; they never touch host plumbing directly.
(function () {
  "use strict";

  // Present only when running inside the game host. hud.html gates its self-
  // running demo loop on this so the demo never fights live telemetry.
  window.__hosted = true;

  // ---- Host -> HUD ----
  // The host calls window.__applyHud('<json>'). We parse and hand off to the
  // mockup's updateHud(). Passing a JSON *string* avoids fragile object-literal
  // escaping on the host side, and is safe if updateHud isn't defined yet.
  window.__applyHud = function (json) {
    var state;
    try { state = JSON.parse(json); } catch (e) { return; }
    if (typeof window.updateHud === "function") {
      try { window.updateHud(state); } catch (e) { /* swallow per-frame errors */ }
    }
  };

  // ---- HUD -> Host ----
  // Fire-and-forget action to the game. Usage in a mockup: game.send('fire_torpedo', { tube: 1 })
  var hasIpc = window.ipc && typeof window.ipc.postMessage === "function";
  window.game = {
    send: function (action, payload) {
      var msg = JSON.stringify({ action: action, payload: payload || null });
      if (hasIpc) { window.ipc.postMessage(msg); return; }
      // wasm/DOM host: dispatch a DOM event the Rust side listens for instead.
      window.dispatchEvent(new CustomEvent("hud-action", { detail: msg }));
    }
  };
})();
