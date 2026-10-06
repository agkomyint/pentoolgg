# v0.8.0 — Professional compositing and reusable image workflows

Build on v0.7's image nodes and immutable operation stacks with the most valuable
professional concepts proven by Photoshop: adjustment layers, reusable masks,
clipping, blending, non-destructive transforms, fill layers, and inspection tools.
Pentool adopts the workflows, not Photoshop's entire surface area. Features must
remain deterministic, scriptable, reviewable in JSON, and suitable for one binary.

## What is worth carrying forward

Photoshop's strongest architectural idea is that edits are independent, reorderable
objects: adjustment layers alter layers below without rewriting pixels; Smart
Objects preserve original content through transforms and Smart Filters; masks make
visibility and filter scope reversible; clipping masks constrain content through
another layer's alpha; and blend modes define compositing mathematically. These map
cleanly to Pentool's scene graph, hashes, history, and structured diffs.

Brush painting, dozens of legacy artistic filters, video, 3D, and bundled generative
models do not fit v0.8.0. AI and content-aware tools remain external-tool operations
whose imported results carry provenance.

## Must ship, in order

- [ ] **1. Add adjustment nodes.** Introduce scene nodes that apply an adjustment
  to siblings below, a referenced group, or explicit target IDs without changing
  source assets. Required adjustments: exposure, brightness/contrast, levels,
  curves, vibrance, hue/saturation, color balance, black-and-white, channel mixer,
  gradient map, invert, posterize, and threshold. Each node has opacity, blend mode,
  enable state, optional mask, stable ID, and a precisely defined color pipeline.

- [ ] **2. Add reusable raster and vector masks.** Support grayscale image masks,
  vector masks, invert, density, feather, transform linking/unlinking, and applying
  one mask to a node or group. Store masks as reusable content-addressed resources.
  White reveals, black conceals, gray provides partial coverage. `mask apply` is an
  explicit destructive bake; detach/delete keeps normal history semantics.

- [ ] **3. Add clipping stacks.** Allow consecutive nodes to clip to the alpha of a
  base node, with explicit stack membership rather than implicit name conventions.
  Define group isolation, base opacity, mask interaction, reordering behavior, and
  broken-reference recovery. This enables photographs inside text, shapes, and card
  frames without duplicating mask geometry.

- [ ] **4. Add a portable blend-mode core.** Specify and implement `normal`,
  `multiply`, `screen`, `overlay`, `darken`, `lighten`, `color-dodge`, `color-burn`,
  `hard-light`, `soft-light`, `difference`, `exclusion`, `hue`, `saturation`,
  `color`, and `luminosity`. Define sRGB/linear conversion, premultiplied-alpha math,
  clamping, group isolation, and pass-through groups. Ship a mode only after exact
  cross-platform conformance fixtures exist.

- [ ] **5. Add non-destructive transform stacks.** Preserve source pixels through
  repeated scale, rotate, skew, perspective, and four-corner transforms. Add a
  bounded mesh warp only after its interpolation and serialization are specified.
  Transform nodes are reorderable and maskable like image operations. Rasterization
  or flattening is always explicit.

- [ ] **6. Add fill nodes.** Implement solid color, linear/radial/conic gradient,
  and content-addressed pattern fills as normal scene nodes. Values may reference
  v0.6.2 design tokens. Define gradient interpolation space, spread, transforms,
  pattern origin, scaling, repetition, and package dependencies.

- [ ] **7. Add non-destructive layer effects.** Start with drop/inner shadow,
  outer/inner glow, stroke, color overlay, gradient overlay, and blur. Effects form
  an ordered, toggleable stack with individual opacity/blend mode and may be shared
  as named effect styles. Separate content opacity from total node opacity.

- [ ] **8. Add selections as temporary queries, saved masks as durable data.**
  Provide rectangle, ellipse, polygon/lasso path, color range, luminosity range,
  node alpha, mask alpha, and boolean add/subtract/intersect operations. Commands
  can turn a selection into a mask, crop, analysis scope, or operation scope.
  Unsaved selections never affect document hashes; saved selections are masks.

- [ ] **9. Add professional inspection tools.** Implement deterministic histogram,
  per-channel statistics, sampled colors, clipping warnings, transparency bounds,
  and before/after comparison. Analysis is read-only, JSON-first, mask/selection
  aware, and able to create palette tokens only through an explicit follow-up.

- [ ] **10. Improve linked-asset workflows.** Add relink, locate-by-hash, embed,
  externalize, collect-for-output, dependency report, stale-source detection, and
  project portability checks. Relinking requires a hash match unless the user
  explicitly imports the bytes as a new asset and reviews affected nodes.

- [ ] **11. Add presets and copy/paste of appearances.** Save operation stacks,
  adjustment settings, effects, masks, fills, and complete appearances as named
  document styles or deterministic `.penpreset` data-only packages. Presets declare
  engine/color-contract compatibility and contain no scripts.

- [ ] **12. Add composite-aware batch and diff.** Batch can create and reorder
  adjustments, masks, clipping stacks, fills, effects, selections-to-mask, and
  transforms in one transaction. Structural diff explains scope and stack-order
  changes; visual diff can isolate the pixels affected by one node.

- [ ] **13. Build editor parity.** Add panels for adjustments, masks, clipping,
  blend modes, effects, histograms, linked assets, and before/after views. Expensive
  previews use cancellable proxies; committed/exported output comes from the shared
  Rust renderer.

- [ ] **14. Harden correctness and performance.** Extend golden tests across group
  isolation, blend modes, masks, adjustment scope, alpha edges, transforms, effects,
  cache reuse, and color math on every release target. Extend benchmarks with deep
  stacks, shared masks, many clipped nodes, large blur radii, and cold/warm caches.
  Enforce memory, temporary-surface, nesting, and operation-count limits.

## CLI direction

```sh
pentool adjustment add photo.pen grade --kind curves --scope group:hero
pentool mask create photo.pen portrait-mask --from node-alpha:portrait
pentool mask attach photo.pen grade portrait-mask --feather 2
pentool clip add photo.pen texture --base headline
pentool object set photo.pen texture --blend overlay --opacity 0.6
pentool effect add photo.pen card shadow --x 0 --y 12 --blur 30 --opacity 0.24
pentool transform add photo.pen screen perspective --quad '...'
pentool inspect histogram photo.pen --scope group:hero --json
pentool asset collect photo.pen ./portable-project --dry-run
```

Every mutating command uses v0.6.2 history, dry run, revision guards, structured
errors, and exactly one transaction.

## Acceptance targets

- Reproduce a professional poster composite using linked images, clipped textures,
  two adjustment nodes, reusable masks, blend modes, gradients, and effects without
  baking any source asset.
- Change the grade globally by editing one adjustment node and limit it locally by
  attaching a reusable mask.
- Repeated transforms do not degrade the source or accumulate resampling damage.
- Collect a project into a portable directory, disconnect the original asset paths,
  and reproduce identical decoded output hashes offline.
- Undo/redo, package round trips, structural diff, and compact serialization retain
  every stack and reference deterministically.
- Golden composite pixels pass on Windows, Linux, Intel macOS, and Apple Silicon;
  performance and peak memory remain within documented budgets.

## Deferred beyond v0.8.0

- Pixel brushes, clone/heal painting, liquify, puppet rigs, and a full raster-layer
  painting engine.
- CMYK/Lab editing and print-proofing until a complete color-management contract is
  designed; v0.8.0 remains explicit sRGB.
- Camera RAW development and lens-profile databases.
- Animation, video timelines, 3D, and neural filters.
- In-core generative fill, background removal, or content-aware synthesis. These
  remain provenance-recorded external tools.

## Acceptance demo

Create a two-page campaign from three hash-verified linked photographs. Apply a
shared curves adjustment through a feathered mask, clip a texture into headline
text, use isolated group blending, add reusable gradient and shadow styles, and
perspective-place a design into a screen mockup. Show histogram analysis, structural
and visual diffs, undo/redo, preset reuse, asset collection, offline package
reproduction, and matching cross-platform golden pixels without baking originals.
