# Asset credits

Placeholder art for the Void & Thunder client. All third-party assets here are
redistributable under their stated licences.

## Ship models — `models/*.glb`, `models/textures/*.png`

**Ultimate Spaceships Pack** by **Quaternius** — https://quaternius.com/packs/ultimatespaceships.html

- Licence: **CC0 1.0** (public domain; no attribution required — credited here anyway).
- The glTF hulls were converted to `.glb`, re-oriented bow-along-+X and up-along-+Z
  for V&T's Z-up world, and normalised to ~44-unit length (see the conversion in
  the project history). Each faction uses one hull plus the pack's matching colour
  variant:

  | Faction | Hull | Texture variant |
  |---|---|---|
  | Corsairs | Executioner | Green |
  | Houses | Imperial | Red |
  | Janissariat | Bob | Orange |
  | Guild | Dispatcher | Blue |
  | Freebooters | Challenger | Purple |

## Skybox — `skybox/phoenix_space_cubemap.png`

Authored for the sibling project *project-phoenix-v2* (same author); reused here.

## Shaders — `shaders/star_surface.wgsl`, `shaders/star_halo.wgsl`

Authored for *project-phoenix-v2* (same author); ported to V&T's Z-up camera.
