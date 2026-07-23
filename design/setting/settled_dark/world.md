# World: The Settled Dark (v0.1, worldspec-core)

The shared grimdark-science-fiction galaxy behind **two** works set in different eras:

- **The Long Night Home** (StorySpec novel) — the *Odyssey era*, a returning war-queen crossing a ruined galaxy.
- **The Void Throne** (GameSpec megagame) — the *Settlement era*, the Charter that seals a conquered peace.

Both reference this world as a `setting` dependency, so the void, the immaterial deep, the
psychic navigators and the imperial faith are authored **once** and stay consistent across
game and prose.

```yaml
world:
  id: settled_dark
  premise: |
    Roughly two hundred inhabited worlds and void-habitats cling to their suns like candles
    in a cathedral. Ships cross the void in days only because psychic navigators feel the
    currents of the immaterial deep. Power is dynastic, religion is old and load-bearing,
    and survival is never clean.

genre_profile:
  id: grimdark_science_fiction
  tech_level: void travel via psychic navigation of an "immaterial deep"; conditioned mass armies; dynastic house rule
  cosmology: material and cruel; the "Throne Eternal" is an old moral weight, not quite a god
  vocabulary_defaults: [the void, immaterial deep, House, Conclave, Charter, Ascendant, sanctification, void-habitat]
  conventions:
    - power is dynastic continuity, control of the void lanes, and the sanctification veto
    - the immaterial deep is not understood; navigators' relationship with it can change
    - every peace is paid for; heroism leaves ethical residue

bible_style_mechanism:
  applies_profiles: [grimdark, gothic]
  scope: [entity_summaries, era_overviews, faction_descriptions]
```

## Consumers

- `long_night_home` (StorySpec) — reads the *Odyssey era* entities.
- `void_throne` (GameSpec) — reads the *Settlement era* entities.
