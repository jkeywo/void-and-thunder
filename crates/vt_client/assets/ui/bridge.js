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
  // Fire-and-forget action to the game. Usage in a mockup:
  //   game.send('select_slot', { slot: 'battery', option: 2 })
  //
  // Actions queue here and the host drains them once a frame. It is a *queue*
  // rather than a direct call because neither host can be called into: Ultralight
  // exposes no `window.ipc` (that is a webview idiom), and on wasm nothing was
  // listening for the DOM event this used to dispatch. Both hosts can, however,
  // read a value back out — so both simply ask.
  var queue = [];
  window.game = {
    send: function (action, payload) {
      if (typeof action !== "string" || !action) { return; }
      queue.push({ action: action, payload: payload || null });
      // Keep a stuck host (no drain) from growing the queue without bound.
      if (queue.length > 64) { queue.shift(); }
    }
  };

  // Drained by the host once a frame; returns everything queued since the last
  // call and empties the queue.
  //
  // The wire format is deliberately not JSON: the host hand-builds its own JSON
  // with `format!` and has no parser, so one line per action as
  // `action|key=value|key=value` is something ~30 lines of Rust can read, and
  // that Rust is unit-testable without a browser. Values are numbers and plain
  // words only — anything that would need escaping does not belong in a control.
  window.__drainActions = function () {
    if (!queue.length) { return ""; }
    var lines = queue.map(function (item) {
      var out = item.action;
      var payload = item.payload;
      if (payload && typeof payload === "object") {
        Object.keys(payload).forEach(function (key) {
          var value = payload[key];
          if (value === null || value === undefined) { return; }
          out += "|" + key + "=" + String(value).replace(/[|\n\r]/g, "");
        });
      }
      return out;
    });
    queue = [];
    return lines.join("\n");
  };
})();
