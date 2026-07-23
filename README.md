# Void & Thunder

Space piracy in the **Settled Dark** — a top-down ship-combat game where you
**aim by steering**. Fly a Corsair sloop through the void lanes and fight
ship-to-ship with directional broadsides, in the spirit of *Assassin's Creed:
Black Flag*'s naval combat and *Rogue Galaxy*'s roving space pirates.

> **Play (web):** https://jkeywo.github.io/void-and-thunder/ *(auto-deploys from `main`)*

## Controls

| Key   | Action                     |
| ----- | -------------------------- |
| **W** | Throttle forward           |
| **S** | Reverse                    |
| **A** | Turn to port (left)        |
| **D** | Turn to starboard (right)  |
| **Q** | Fire **port** broadside    |
| **E** | Fire **starboard** broadside |
| **Space** | Brace — cut incoming damage |
| **B** | Board a crippled enemy alongside (loot it) |
| **R** | Restart after a run ends   |

**Gamepad** (Black-Flag scheme): **RT/LT** throttle & reverse · **left stick**
steer · **LB/RB** port/starboard broadside · **X** brace · **A** board · **Start**
restart.

You can't strafe — turn the hull to bring a broadside to bear. Presenting your
beam is the skill. Batter an enemy's hull low enough and it's **crippled** (grey,
drifting): pull alongside and press **B** to board and plunder it, or finish it
with fire. Clear every wave to win.

Sound effects are synthesised procedurally — played through Bevy audio on native,
and through a WebAudio shim (`index.html`) on the web, where Bevy audio is disabled.

## Architecture

A Cargo workspace with a hard split between simulation and presentation:

```
crates/
  vt_sim/     Simulation — ship physics & combat. Built on Bevy ECS
              (logic-only subcrates: no render/window/audio), so it runs
              headless and is unit-tested. Owns ALL game rules.
  vt_client/  Bevy front end — window, renderer, camera, input. Mounts
              vt_sim's SimPlugin. Owns NO game rules. Native + wasm.
design/
  setting/    Vendored snapshot of the "Settled Dark" worldspec (see SOURCE.md).
pasm/spec/    PASM architecture model — the intended shape of the codebase,
              checked against the Rust implementation. See pasm/spec/README.md.
docs/
  mvp-plan.md                          The MVP roadmap.
  research/ship-combat-references.md   Study of the reference games.
```

Why the split: the sim is the game; the client is one way to see it. Keeping the
sim renderer-agnostic makes it fast to compile, deterministic (fixed timestep),
testable without a window, and portable to another front end later.

## Develop

```bash
# Run the native client
cargo run -p vt_client

# Faster iterative debug builds (native only, dynamic linking)
cargo run -p vt_client --features fast-compile

# Test the simulation (fast, headless)
cargo test -p vt_sim

# Run the web client locally (needs `trunk` + the wasm target)
rustup target add wasm32-unknown-unknown
cargo install trunk
trunk serve            # http://localhost:8080
```

## Deploy

Pushing to `main` triggers `.github/workflows/pages.yml`, which builds the wasm
client with Trunk and publishes it to GitHub Pages. Enable it once under
**Settings → Pages → Build and deployment → Source: GitHub Actions**.

## Design tooling (PASM)

The intended architecture is modelled in `pasm/spec/` and checked against the
code, so changes can't quietly grow a second way of doing something that already
exists. Requires [`uv`](https://docs.astral.sh/uv/):

```bash
uv run pasm validate pasm/spec     # check the model
uv run pasm scan pasm/spec --json  # scan the Rust code against it
```

## Setting

Set in the **Settlement Era** of the *Settled Dark* — a conquered galaxy of ~200
worlds held together by psychic navigation of the "immaterial deep". You're a
Corsair preying on the House, Janissariat and Guild ships that run the lanes. The
canon is vendored (read-only) under `design/setting/`; see its `SOURCE.md`.

## Status

Playable vertical slice (fly + fire on stationary targets), native and web. The
full core loop — enemy AI, a star system, spawning waves, win/lose — is planned
in [`docs/mvp-plan.md`](docs/mvp-plan.md).
