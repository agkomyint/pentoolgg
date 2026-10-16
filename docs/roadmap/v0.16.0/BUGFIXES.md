# v0.16.1 bug fixes

This patch release fixes two Studio raster-painting problems reported after
v0.16.0:

- Release builds now embed the browser renderer compiled for
  `wasm32-unknown-unknown`. CI smoke-renders a painted raster fixture through
  the WASM exports and verifies that its pixels affect the output. Native-only
  scene3d cache controls are excluded from the WASM build.
- Raster pointer mapping and the live brush overlay now include the raster
  node's own transform. The overlay follows the selected brush kind: hard-round
  no longer previews as a soft brush, soft-round uses the engine's smoothstep falloff,
  and pixel brushes preview as square dabs.

Focused checks: `bun test src/lib/model.test.ts`, `bun run typecheck`, and
`node scripts/smoke-wasm-raster.mjs ... docs/fixtures/raster-v6.pen`.
