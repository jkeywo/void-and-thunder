# Status ring — design brief

The under-ship status ring is deliberately a plain SVG so its look can be
authored somewhere better than a Rust file. This is the brief to hand to Claude
Design (or any designer) to get a drop-in replacement.

**What to replace:** the `<template id="ringTemplate">` block and the
`/* ---- Status rings ---- */` CSS section in
[`crates/vt_client/assets/ui/hud.html`](../crates/vt_client/assets/ui/hud.html).
Nothing else should need to change — the script that drives it is written
against the class names below, not against the shapes.

**Do not change** the `viewBox`, the class names, or the radii-to-constants
pairing without also updating `C_HULL` / `C_SHIELD` / `C_GUN` in the same file.
The full contract is documented in the `RING CONTRACT` block at the top of
`hud.html`; the prompt below restates the parts a designer needs.

---

## The prompt

> I need an SVG component for a video game HUD. It is a **status ring** that is
> drawn on the ground beneath a spaceship, seen from a camera about 50° above
> the ground plane. Think of a circle painted on the deck around the ship.
>
> ### The game
>
> Grimdark space piracy — sailing-age naval combat transposed into space. You
> captain a corsair sloop, fight with broadsides, and board crippled hulks. The
> existing HUD is a **tarnished amber CRT** aesthetic: scratched metal bezels,
> phosphor-amber text (`#ffb200`) with a soft glow, dark screens, hazard
> stripes, a little grime and scanline flicker. Warnings go orange (`#ff7a18`),
> critical goes red (`#ff2f14`), shields are a cold electric blue
> (`#96d7ff`), torpedo tubes are a pale green (`#78ffbe`). The ring should feel
> like part of that instrument suite — machined, functional, slightly worn —
> not like a clean modern app UI.
>
> ### Hard technical constraints
>
> These are not stylistic preferences; the code will not work otherwise.
>
> 1. **One SVG, `viewBox="0 0 200 200"`.** The ring is centred at `(100,100)`
>    with an outer radius of `100`. Nothing may extend beyond that radius.
> 2. **Author it flat and circular.** The game applies a CSS `matrix()` that
>    lays it onto the ground plane in perspective. Do not draw an ellipse or
>    fake any perspective — you would get it twice.
> 3. **It will be foreshortened, hard.** Vertical extent is squashed to roughly
>    60% at rest, and much more when the camera lifts overhead. **Use stroked
>    arcs, not filled bands** — a thin filled ring collapses into an unreadable
>    smear when squashed. Keep strokes ≥ 3 units wide.
> 4. **No text, no numbers, no icons with a "right way up".** The ring rotates
>    with the world and gets squashed; anything readable becomes unreadable.
> 5. **Angles are clockwise from east** (standard SVG). Local `+x` is the
>    world's `+x`, local `+y` is the world's `−y`.
> 6. **No CSS animations, transitions, or filters that animate.** This page is
>    rasterised into a texture every frame on desktop, and anything
>    continuously animating forces a repaint of the whole HUD. Static
>    `drop-shadow` for glow is fine.
>
> ### The five bands, and how they are driven
>
> Every animated element is driven by the script writing `stroke-dasharray` and
> `stroke-dashoffset` on it, so **each must be a full `<circle>`** at the right
> radius — not an arc path. The script turns it into an arc. Keep these exact
> class names and give each one a radius; tell me the radii you chose.
>
> | Class | What it shows | Behaviour |
> |---|---|---|
> | `.ringTrack` | Hull, empty portion | Full circle, dim, never changes |
> | `.ringHull` | Hull remaining | Arc shrinks as hull is lost. Gets `.warn` below 50%, `.crit` below 25% — style both |
> | `.ringShieldTrack.fore` / `.aft` | The two shield arcs, unpowered | Fixed half-circles, dim, always visible even at zero charge |
> | `.ringShieldFore` / `.ringShieldAft` | Shield charge, fore and aft | Each covers up to 180°, growing outward from the middle of its half |
> | `.ringGunArc.port` / `.stbd` | Broadside firing arcs | ~135° each, centred on the ship's beam. Gets `.reloading` — style that as clearly "not ready" |
>
> Plus a `<g class="ringTubes">` — leave it **empty**; the script generates one
> `<line class="ringTube">` per torpedo tube inside it, spread across the bow.
> Style `.ringTube` and `.ringTube.empty` (an unloaded tube).
>
> Two whole-ring modifiers to style:
> - `.ring.them` — an enemy ship's ring. Should read as quieter and secondary.
> - `.ring.noShield` — a hull with no shields fitted; the shield band must
>   vanish entirely, *not* render as two empty arcs.
>
> ### What I want back
>
> A single self-contained block of SVG markup plus its CSS. Decorative extras
> are welcome — tick marks, a bezel, registration marks, wear — as long as they
> obey the constraints above and sit inside radius 100. Show me it flat, and
> also show me what it looks like scaled to 60% vertically, so I can check it
> survives the foreshortening.

---

## After you get it back

1. Paste the markup over the `<template id="ringTemplate">` contents and the
   CSS over the `/* ---- Status rings ---- */` block.
2. If the radii changed, update `R_HULL_MAX`, `R_SH_MIN`, `R_SH_MAX`, `R_GUN`
   and `R_TUBE` in `hud.html` to match the `r=` attributes in the markup.
3. If a band's *mechanism* changed — the current design drives hull by radius
   and shields by stroke width rather than by arc length — the driving code in
   `__applyRings` has to change with it. The `RING CONTRACT` block at the top of
   `hud.html` documents what each class expects; keep it true.
4. Open `crates/vt_client/assets/ui/hud.html` directly in a browser — it runs a
   demo feed with two rings when opened standalone.
5. `cargo run -p vt_client` to see it on the plane.
