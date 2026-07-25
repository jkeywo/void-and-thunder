@RTK.md

# Void & Thunder — Agent Guide

Space piracy in the Settled Dark: a top-down ship-combat game where you aim by
steering. Broadsides, EMP, torpedo locks, microwarp, boarding — played native
or in the browser. The setting is a vendored read-only snapshot under
`design/setting/` (see its `SOURCE.md`: the world owns fictional truth, the
game owns mechanics; edit upstream and re-copy, never fork the lore here).

## Tech stack

| Layer | Technology |
|---|---|
| Simulation core | Rust, `crates/vt_sim` — Bevy ECS subcrates only (no renderer/window/asset); owns ALL game rules |
| Client | Bevy 0.19, `crates/vt_client` — window, renderer, camera, input, HUD, audio; owns NO game rules |
| Game data | RON under `crates/vt_client/assets/data/` via a generic `RonAssetLoader`; `vt_sim` owns the types and never touches a file |
| HUD | Authored web page `assets/ui/hud.html` with a JSON contract; wasm overlay / native Ultralight (`native-html-hud` feature) |
| Architecture model | PASM — YAML spec under `pasm/spec/`, tool pinned from vellum |
| Shared crates | none yet — this repo is the designated first consumer for vellum-corpus (scenarios), vellum-perf (profiling), and vellum-compose (data editor) |
| CI | fleet-ci caller (`.github/workflows/ci.yml`) → pasm gates, clippy `-D warnings`, tests, Trunk build, Pages deploy |

## Project rules

- Every sim system runs in `FixedUpdate` through the ordered `SimSet` chain
  (`plugin.rs`); presentation reads interpolated poses and never writes sim
  state.
- Gameplay numbers live in RON, not Rust consts. `Default` impls must
  reproduce the shipped RON exactly — a test asserts it — so missing data
  degrades instead of drifting.
- Hot reload (`--features hot-reload`) and replay determinism do not mix;
  that trade-off is a recorded decision in the spec. Don't disturb the
  RonAssetLoader/DataPlugin path — the data editor builds on it.
- Ships are one uniform entity shape (`ship_bundle`): player and AI differ
  only in which system writes their `PilotIntent`. The headless `Harness`
  depends on this — keep it true.
- `lcg_next` stays hand-rolled: it is eight lines of cosmetic spawn jitter,
  and vellum-rng would change spawn feel for zero benefit.
- Read and update `pasm/spec/` before or alongside every structural change;
  record accepted choices in `pasm/spec/core/decisions.yaml`.

## PASM — keep it up to date

1. Model first, then build — spec entities before Rust for a new system.
2. Record decisions in `pasm/spec/core/decisions.yaml` as you make them.
3. `uv run pasm validate pasm/spec` after any model change; fix before commit.
4. `uv run pasm scan pasm/spec --json` gates CI — keep implementation
   mappings (paths/symbols/tests) current.
5. Never leave dead spec — removing a system updates its declarations.

## Common commands

```bash
# CI gates — run all of these before calling work done
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
uv run pasm validate pasm/spec

# Run the game (run.bat wraps the same: bare/fast/hud/release/test modes)
cargo run -p vt_client
cargo run -p vt_client --features fast-compile        # dynamic linking
cargo run -p vt_client --features hot-reload          # RON file watching

# Web build
trunk serve                        # http://localhost:8080
trunk build --release              # CI ships this with --public-url /void-and-thunder/
```

## Vellum — the shared foundation

This repo pins vellum by rev in `pyproject.toml` (pasm) and the `uses:` line
of `.github/workflows/ci.yml`. A vellum bump PR aligns both and touches
nothing else. Local override etiquette: vellum `docs/handbook/local-dev.md` —
never committed active.
