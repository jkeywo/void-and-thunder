# Void & Thunder — PASM specification

This directory is the source of truth for Void & Thunder's system boundaries
and gameplay design. [PASM](https://github.com/jkeywo/pasm) reads the model here
and checks the Rust implementation against it, so a change to one part of the
game can't quietly grow a second, parallel way of doing something that already
exists.

- `core/foundation.yaml` — the entities of the game: the simulation systems
  (movement, combat), the `SimPlugin` wiring, and the client renderer/input
  layer, each mapped to the code that implements it (`paths`, `symbols`,
  `tests`) and carrying a lifecycle `status`.
- `core/decisions.yaml` — decisions made while building: technology choices and
  interpretation calls, each with rationale.

Add or update the model for a system **before or alongside** its implementation,
and keep `uv run pasm validate pasm/spec` green — CI runs it on every push.

```sh
uv run pasm validate pasm/spec     # check the model is well-formed
uv run pasm scan pasm/spec --json  # scan the Rust implementation against it
```
