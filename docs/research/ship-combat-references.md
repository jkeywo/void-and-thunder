# Research — ship combat references

Reference study for Void & Thunder's core loop: **flying a ship and fighting
ship-to-ship, as space piracy in the Settled Dark.** Three touchstones, and what
we take from each.

---

## 1. Assassin's Creed IV: Black Flag — the naval model we're cloning

Black Flag's naval combat (revisited in the 2026 *Black Flag Resynced* remake) is
the primary reference for **controls and weapon feel**. The design is built
around *directional weapons on a ship that must be steered to aim* — turning is
aiming.

**Weapon systems, by mount direction:**

- **Broadside cannons (sides)** — the bread-and-butter weapon. You steer so the
  target is off your beam, then fire a volley. Ammo variants: round shot
  (flexible), heavy shot, and heated shot (multiple fiery volleys, high damage).
- **Chain shot (front)** — slows a fleeing enemy so you can close, ram, or line
  up a broadside.
- **Mortars (indirect)** — lobbed from above onto a target area at range; a
  secondary mode drops many projectiles with manual placement.
- **Explosive barrels (rear)** — dropped as traps behind you.
- **Ram (front)** — closing weapon after a chain shot.

**Defence:**

- **Brace** — hold a button as a hit lands to cut incoming damage.
- **Perfect Brace** — a timed parry that nullifies a hit entirely.

**The core tactical idea:** *"Weapons are locked behind directional use, making
U-turns important to set up weapons at a moment's notice."* You are always
turning the whole ship to bring the right weapon to bear. Combat is a positioning
duel, not a twin-stick shooter.

**Boarding** ends a weakened enemy: pull alongside, then a short action to take
the ship and loot it.

### What Void & Thunder takes

- **Directional weapons = aiming by steering.** Our `Broadside` fires port or
  starboard; you must present your beam to the target. Already implemented:
  `FireOrders { port, starboard }` + `broadside_volley` throwing balls out the
  side, inheriting ship momentum. **This is the heart of the game.**
- **Turning-is-aiming handling.** `helm_step` gives momentum, a top speed and a
  turn rate — you commit to arcs, you can't strafe. Keep it that way.
- **A kit of directional tools** (roadmap): chain shot (front, slow), a lob/
  mortar analogue, drop-mines (rear). Each tied to a facing, so the fantasy is
  managing which weapon your current heading enables.
- **Brace** (roadmap): a damage-reduction defensive button, later a timed parry.
- **Boarding** (roadmap): the finisher on a crippled hull — the piracy payoff.

Sources:
[Ubisoft deep dive](https://www.ubisoft.com/en-us/game/assassins-creed/news/1QoM4qTIi9ERlzDOrxeRgK/assassins-creed-black-flag-resynced-deep-dive-into-the-naval-gameplay),
[Mobalytics naval guide](https://mobalytics.gg/news/guides/assassins-creed-black-flag-resynced-naval-combat-guide),
[TheGamer combat tips](https://www.thegamer.com/assassins-creed-black-flag-resynced-combat-guide/).

---

## 2. Assassin's Creed Rogue — the same model, extended

AC Rogue reuses Black Flag's naval engine and adds ideas worth stealing for a
*space* pirate game:

- **Environmental hazards** (icebergs) you weave through and shoot to damage
  enemies — the arena fights back. Our analogue: asteroids, debris fields,
  gravity wells near the star that bend fast movement.
- **On-deck defensive weapon** (the puckle gun) against boarders and swarms — a
  point-defence idea for when small craft close on you.
- **Being hunted.** Rogue casts you as the pursuer *and* the pursued. Good
  framing for wave spawns that hunt the player across a system.

### What Void & Thunder takes

- **The arena is a combatant** — hazards (asteroids/gravity) in the `star-system`
  playfield, not an empty void.
- **Point-defence vs. swarms** — a short-range auto-weapon once small enemy craft
  exist.

Source:
[Sportskeeda — AC games with best combat](https://www.sportskeeda.com/esports/assassins-creed-games-best-combat-gameplay).

---

## 3. Rogue Galaxy (Level-5, PS2) — the fantasy, not the controls

Rogue Galaxy is a space-pirate action-JRPG. We take almost nothing mechanical
from its on-foot combat, but it nails the **fantasy and structure** we want
around the ship fights:

- **A crew of charismatic space pirates** aboard a **home ship that is a hub** —
  the crew operates it and the story advances there between adventures.
- **Planet/system hopping** across a varied galaxy, framed as roving piracy.
- **Real-time action combat** with **combos that chain between enemies** — finish
  one and you snap to the next nearest. A feel worth echoing when a broadside
  volley sweeps across a cluster of ships.

### What Void & Thunder takes

- **The framing:** you are a charismatic Corsair captain roving the lanes of the
  Settled Dark; the ship is your home and hub between engagements.
- **System-hopping structure** (roadmap): the MVP is one system; the loop is
  built so systems become the unit of content.
- **Sweep-through payoff:** volleys that catch several ships in an arc should feel
  like a chained hit, not isolated shots.

Sources:
[RPGSite preview](https://www.rpgsite.net/preview/2780-rogue-galaxy-preview),
[Rogue Galaxy Wiki](https://roguegalaxy.fandom.com/wiki/Rogue_Galaxy).

---

## Synthesis — the design line for Void & Thunder

> **Black Flag's directional-weapon helm duel, set among Rogue Galaxy's roving
> space pirates, in the grimdark void of the Settled Dark.**

Non-negotiable core (the thing every milestone protects):

1. **You aim by steering.** Momentum + turn rate + directional broadsides. No
   strafing, no free-aim turret. Presenting your beam is the skill.
2. **Space, not sea, but read as sea.** Top-down, ships bank into turns, the void
   is dark and the lanes are dangerous. 2D now; the feel survives a later move
   to 3D if we want it.
3. **Piracy is the point.** Combat exists to take ships — cripple, board, loot.
   The MVP proves the fight; the finisher and loot make it piracy.
