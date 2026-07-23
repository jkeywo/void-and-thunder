# Setting source — The Settled Dark

`settled_dark/` is a **read-only, vendored snapshot** of the shared worldspec
package that defines this game's setting.

- **Canonical source:** `C:\AnalogueGames\AIWriting\GameProjects\_shared_worlds\settled_dark`
- **Spec it conforms to:** `worldspec-core/0.6`
  (`C:\AnalogueGames\AIWriting\DSL\worldspec_core_spec_v0_6.md`)
- **Snapshot taken:** 2026-07-23, revision `settled-dark-0.1` (development)

Canon lives upstream. Edit the world there, then re-copy here — do not fork the
lore in this repo. The game **references** these entities (names, factions,
cosmology) and layers its own *mechanics* on top; per the worldspec contract,
the world owns fictional truth, the game owns mechanics.

## What Void & Thunder takes from the world

Void & Thunder is set in the **Settlement Era** (the era of *The Void Throne*):
a conquered galaxy of ~200 inhabited worlds and void-habitats, held together by
psychic navigation of the **immaterial deep**. The player is a **Corsair** — a
void pirate working the lanes the great powers depend on.

Factions in the sim (`vt_sim::components::Faction`) map to setting entities:

| Sim faction   | Setting entity                                   |
| ------------- | ------------------------------------------------ |
| `Corsairs`    | The player and allied pirates (new to the world) |
| `Houses`      | The Conclave of Thrones — dynastic House patrols |
| `Janissariat` | Janissariat High Command warships                |
| `Guild`       | The Ductus Guild — psychic navigators/couriers   |
| `Freebooters` | Unaligned raiders, hostile to all               |

See `design/setting/settled_dark/world.md` and `entities/settlement_era.md` for
the canon these are drawn from.
