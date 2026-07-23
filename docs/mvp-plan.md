# Void & Thunder — MVP plan

**Goal:** *Fly a ship around a star system and engage in ship-to-ship combat
against spawning enemies* — a complete, satisfying core loop, playable in the
browser and native.

The design line is fixed in [`research/ship-combat-references.md`](research/ship-combat-references.md):
**Black Flag's directional-weapon helm duel, among Rogue Galaxy's roving space
pirates, in the void of the Settled Dark.** You aim by steering. No strafing.

---

## Definition of done (the MVP is these six sentences)

1. I spawn as a Corsair sloop in a bounded star system with a star, a couple of
   stations as landmarks, and a soft boundary that turns me back.
2. I fly with a weighty helm — throttle, reverse, and turn-to-aim — and the
   camera keeps me in frame.
3. Hostile House ships **spawn in waves** and hunt me.
4. They **steer to present a broadside** and fire when their beam lines up; so do
   I. Cannonballs damage hulls; friendly fire is ignored.
5. Ships that lose their hull are destroyed, with a hit/death effect and sound.
6. The encounter **resolves** — I clear the waves (win) or my hull reaches zero
   (lose) — and I can restart. A HUD shows my hull and the wave.

If all six are true in both the native and the web build, the MVP ships.

---

## What already exists (the scaffold / vertical slice)

Implemented and tested in `crates/vt_sim` + `crates/vt_client`:

- **Helm physics** — `helm_step` (thrust along bow, turn rate, drag, top speed),
  integrated by `movement_system`. *(unit-tested)*
- **Broadsides** — `Broadside` + `FireOrders`; `broadside_volley` throws a spread
  of momentum-inheriting cannonballs out port/starboard on a cooldown. *(tested)*
- **Projectiles & damage** — flight, TTL expiry, circle-overlap collision,
  faction-filtered damage, hull destruction. *(tested)*
- **Enemy AI** *(M1 — done)* — `desired_helm` + `ai_system`: `AiController` ships
  find the nearest hostile and flee / pursue / present-a-beam-and-fire. *(tested,
  incl. a headless ECS-schedule integration test)*
- **Star system** *(M2 — done)* — `SystemBounds` + `bounds_return`/`bounds_system`:
  a soft inward spring turns ships back at the edge (no wall). Client draws a
  central star + stations as circular `Landmark`s and a parallax starfield. *(tested)*
- **Spawn director + encounter** *(M3 — done)* — `director_system` sends
  escalating waves at the `Protagonist`; `Encounter` tracks wave / enemies /
  `Outcome`; client maps it to `Playing`/`GameOver` with R-to-restart and a HUD
  line. `ship_bundle` is the single ship constructor. *(tested)*
- **`SimPlugin`** — all of the above ordered in `FixedUpdate` (`Director → Ai →
  Movement → Bounds → Weapons → Resolution`).
- **Client** — window, camera-follow, sprites auto-attached to sim entities,
  WASD helm + Q/E broadsides; spawns the player + 3 stationary House targets.
- **Pipeline** — native `cargo run`; web via Trunk; auto-deploy to GitHub Pages
  on push; CI runs tests + PASM.

So today you can fly and blow up stationary hulks. The MVP turns that into a
real encounter. **Everything below is the remaining work.**

---

## Milestones

Each milestone is a vertical slice that leaves the game playable. Tackle in order.

### M1 — Enemy AI (make the hulks fight back)  ·  `enemy-ai` ✅ DONE
The single biggest step: a controller that writes `Helm`/`FireOrders` for
non-player ships. **Shipped** — `crates/vt_sim/src/ai.rs` (`desired_helm` +
`ai_system`, `SimSet::Ai`). The list below is the design it was built to.

- New `crates/vt_sim/src/ai.rs`, a system in a new `SimSet::Ai` **before**
  `Movement`.
- Behaviour (a small state intent, no need for a full FSM yet):
  - **Pursue:** steer toward the nearest hostile; throttle up when far.
  - **Present broadside:** when inside engagement range, steer so the target sits
    ~90° off the bow (aim the beam), not head-on.
  - **Fire:** when the target is within a firing arc off port or starboard and in
    range, set the matching `FireOrders`.
  - **Disengage:** below a hull threshold, turn away and run.
- Pure helper `desired_helm(self_pos, self_heading, target_pos, ...) -> (Helm, FireOrders)`
  so the aiming maths is unit-tested without a `World`.
- **Done when:** the 3 House ships chase and shoot the player; a fight is losable.

### M2 — Star system playfield  ·  `star-system` ✅ DONE
Somewhere to fight. **Shipped** — `crates/vt_sim/src/world.rs` (soft bounds) +
client landmarks and starfield. The list below is the design it was built to.

- A `SystemBounds` resource (radius) + a soft-return system: past the edge,
  damp/steer velocity back inward (no hard wall).
- Static landmarks: a central star and 1–2 stations/planets (sprites), purely
  visual + spatial anchors for now.
- Parallax starfield background so motion reads.
- **Done when:** flying to the edge turns you back; the space feels like a place.

### M3 — Spawn director & encounter state  ·  `spawn-director` ✅ DONE
Turn skirmish into an encounter. **Shipped** — `crates/vt_sim/src/spawn.rs`
(director, waves, `Encounter`/`Outcome`) + client states/restart/HUD. The list
below is the design it was built to.

- `SpawnDirector` resource: spawns hostile waves at the system edge around the
  player, wave N bigger/tougher than N−1, next wave when the current is cleared.
- `Encounter` state: `Ships remaining`, `Wave`, and an outcome
  (`InProgress | Cleared | PlayerDestroyed`).
- Bevy `States` for `Playing` / `GameOver`; restart on a key.
- **Done when:** waves keep coming, clearing all = win, dying = lose, R restarts.

### M4 — Game feel & HUD (make it read and feel good) ✅ DONE (visuals)
The layer that makes 1–3 satisfying. **Shipped** — sim `ShipHit`/`ShipDestroyed`
messages drive client hit sparks, explosions, muzzle flashes and screen shake
(`CameraRig`); a hull gauge + wave/outcome HUD; enemy damage tint. **SFX is the
one deferred piece** — it needs audio assets (no bundled sounds yet).

- **HUD** (`bevy_ui`/`Text`): hull bar, current wave, ships remaining, outcome
  banner.
- **Juice:** muzzle flash, hit spark, ship-death flash/particles, brief screen
  shake on hits; broadside/hit/explosion SFX.
- **Readability:** health tint on enemy hulls; off-screen enemy markers.
- **Done when:** a new player understands the fight without instructions.

### M5 — Piracy finisher (optional for MVP, defines the game)
The thing that makes it *piracy*, not just a shooter. Pull in if M1–M4 land early.

- **Brace** (Black Flag): a held key that reduces incoming damage.
- **Cripple + board:** a hull below a threshold is disabled, not destroyed; close
  alongside to board → loot → destroy. First taste of the piracy loop.

---

## Ordering & estimate

```
M1 Enemy AI ─▶ M3 Spawn director ─▶ M3 encounter/states
      │              ▲
      └─▶ M2 System playfield ──────┘ ─▶ M4 Feel & HUD ─▶ (M5 finisher)
```

M1 first — a fight that fights back is the riskiest, highest-value unknown. M2
can run in parallel. M3 needs M1. M4 polishes the whole. M5 is the stretch that
turns the MVP into "the game".

Rough sizing (solo): M1 ~2–3 days, M2 ~1 day, M3 ~1–2 days, M4 ~2–3 days,
M5 ~2 days. **Core MVP (M1–M4): ~1.5–2 weeks.**

---

## How we keep it honest

- **PASM:** M1–M3 are already modelled as `proposed` entities in
  `pasm/spec/core/foundation.yaml`. Flip each to `implemented` with its
  `paths`/`symbols`/`tests` as it lands, and keep `uv run pasm validate` green.
- **Tests:** every new pure helper (`desired_helm`, bounds return, wave sizing)
  gets unit tests, matching the pattern in `ship.rs`/`combat.rs`. The sim stays
  headless-testable — that's why it's a separate crate.
- **Both targets:** each milestone must run native *and* on the deployed web
  build before it's called done.

---

## Explicitly out of scope for the MVP

Deferred so the core loop stays the focus: multiple star systems / travel between
them; the crew/hub ship and story; ship upgrades, economy and progression;
multiplayer; 3D; save/load. These are natural next steps once the loop is fun —
the architecture (separate sim crate, ECS, systems as content) is built to grow
into them.
