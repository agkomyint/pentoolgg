# v0.10.0 — Rust-native pixel editing and retouching

> **Status (dev, unreleased, local commits, not yet pushed or CI-verified):**
> 0 of 14 items are ticked; items 1-10 are implemented and items 10 and 13 are partial. Boxes stay unticked until every
> item has implementation, tests, docs and hosted Linux/Windows/macOS CI evidence.
>
> | # | Item | State |
> |---|------|-------|
> | 1 | Spec, schema, fixtures | Implemented: `docs/raster-paint-v1.md` + schema, valid/missing-tile/corrupt-tile fixtures, replayable checkpoints with auto-rolling journal, `raster verify [--replay]`, `raster repair --strategy replay\|transparent`, node `engine` with view-only rule for newer engines. Awaiting hosted CI evidence before ticking. |
> | 2 | Raster layers | Implemented: add, resize, crop, trim, clear, duplicate, merge-down, explicit rasterize; groups, masks, clipping, effects, components (tiles retained through snapshots and fallbacks), packages, tile-aware diff, undo, dry run, multi-page selection (`tests/raster_layers.rs`). Awaiting hosted CI evidence before ticking. |
> | 3 | Brush engine | Implemented: hard-round, soft-round, pixel, calligraphic and textured stamp (content-addressed `brush_tips` stored as tiles), size, hardness, spacing, opacity, flow, angle, roundness, seeded scatter, EMA `smoothing`, `buildup` on/off; journals pin tip digests; cross-platform tile-hash golden (`tests/raster_brush.rs`). Awaiting hosted CI evidence before ticking. |
> | 4 | Input normalization | Implemented: `src/raster/input.rs` normalizes events with position, pressure and optional tilt, azimuth, twist and velocity (all-or-none per stroke). Velocity is derived from `t`, sub-0.5 px events are folded, and the last event is always kept. Journals hold canonical samples without timestamps. Brush `dynamics` maps inputs to size, flow, roundness and angle through explicit piecewise-linear curves with stable fallbacks. Existing journals and hashes are unchanged (`tests/raster_brush.rs`). Awaiting hosted CI evidence before ticking. |
> | 5 | Erase and local blending | Implemented: pixel eraser, clear-to-transparency (`raster clear` and the `erase` blend), background eraser, smudge, blur, sharpen, dodge, burn, sponge and color-replace as stroke blends with defined sampling radius, accumulation, alpha behavior, clamped edge mode, tolerance and tone range; misapplied parameters are errors; replayable from the journal (`tests/raster_brush.rs`). Selection and mask interaction waits for item 9. Awaiting hosted CI evidence before ticking. |
> | 6 | Clone stamp | Implemented: aligned and non-aligned clone strokes from the target layer or another raster layer with rotation and scale; strokes pin the exact source tiles in the journal so later source edits cannot reinterpret them; replay, verify and dry-run covered (`tests/raster_clone.rs`). Not yet: sampling the composite of other scene content (waits for stamp-visible, item 10), and live preview (item 12). Awaiting hosted CI evidence before ticking. |
> | 7 | Heal | Implemented: `heal` blend (texture from the source, tone from the surroundings, bounded deterministic solver, algorithm recorded in the journal), `heal-stroke` and automatic-source `heal-spot`; replay, determinism, limits and dry-run covered (`tests/raster_heal.rs`). Not yet: patch healing (needs selection transforms, item 9). Awaiting hosted CI evidence before ticking. |
> | 8 | Flood fill and contiguous selection | Implemented: `fill` and `select-wand` with tolerance, 4/8 connectivity, global mode, gap policy, transparency handling, anti-aliased edges, bounded allocation, and refine modes (replace/add/subtract/intersect); an active selection limits strokes, clone, heal and fill with the selection pinned in the journal for replay (`tests/raster_fill.rs`). Not yet: composite sampling scope (item 10), mask output (item 9). Awaiting hosted CI evidence before ticking. |
> | 9 | Selections and transforms | Implemented: `select-marquee` (rect/ellipse), `select-lasso`, `select-quickmask`, `select-modify` (feather, expand, contract, smooth, border, grow, similar, invert), saved selections (`select-save/load/delete`), and for selected pixels `lift` (copy/cut to a new layer, alpha kept), `move-pixels`, `transform-pixels` and `paste`; selections follow moved and transformed pixels (`tests/raster_select.rs`). Not yet: patch healing (item 7 remainder). Awaiting hosted CI evidence before ticking. |
> | 10 | Layer operations | Implemented: trim, canvas resize (crop past bounds), merge down, explicit rasterization, `rotate`/`flip` (lossless, selections follow), `merge-visible`, `stamp-visible`, `flatten` (`tests/raster_compose.rs`). Not yet: the "below" clone source and composite flood scope that build on stamp-visible. Awaiting hosted CI evidence before ticking. |
> | 11-12 | Presets, editor UI | Not started. |
> | 13 | CLI and batch | Partial: `pentool raster ... add/info/clear/checkpoint/stroke/verify/repair/resize/crop/trim/duplicate/merge-down/rasterize/tip-add/tip-remove/tips` with dry-run and limits. Missing: presets, selections/masks, retouch ops, batch. |
> | 14 | Performance, fuzz, goldens | Not started (one replay-hash test exists). |

Complete the missing hands-on raster workflow after v0.7.0 image operations,
v0.8.0 compositing, and v0.9.0 model-assisted editing. Add paintable raster layers,
brushes, erasing, clone/heal, patching, and local retouching through one bounded,
deterministic Rust engine. Human edits and AI-produced assets use the same masks,
selections, history, compositing, inspection, and export paths.

This milestone targets the daily pixel-editing work for which users still reach
for Photoshop. It does not copy Photoshop's UI or file format. Pentool keeps edits
scriptable, recoverable, inspectable, and portable in a readable `.pen` document.

## Product contract

- Painting is a first-class scene workflow, not opaque mutation of an imported
  source. Original assets remain immutable until an explicit destructive bake.
- The authoritative brush, retouch, tile, and compositing engines are Rust code
  shared by the CLI, browser editor, server API, batch system, and renderer.
- A completed document renders offline from materialized local data. Brushes,
  plugins, models, and external services are never required merely to reopen it.
- Input events are normalized into deterministic strokes. Replaying a committed
  stroke with the same engine version and inputs produces its normative tile set.
- Long sessions remain bounded. History, dirty regions, tile caches, checkpoints,
  temporary surfaces, and brush samples all have explicit memory and disk limits.
- Pixel editing respects page, layer, group, selection, mask, lock, blend, opacity,
  color-space, transaction, and revision semantics already established by v0.7.

## Raster layer and tile model

Add a raster-paint node backed by immutable, content-addressed tiles rather than a
single image rewritten after every stroke. The node declares logical bounds, tile
size, pixel format, color contract, sparse tile map, and an ordered edit journal.
Unpainted tiles are transparent. Identical tiles deduplicate across layers,
checkpoints, history, packages, and documents where the cache contract permits.

The tile size and edge behavior are format contracts, not tuning accidents. Tile
seams are forbidden for filtered brushes, healing, transforms, and export. Dirty
regions include every kernel halo and sampling dependency. Cache entries may be
discarded; committed tile hashes and journal/checkpoint data may not.

Stroke records contain a stable ID, brush-engine version, preset reference,
canonical input samples, chosen color, blend mode, target, selection/mask hashes,
seed where applicable, and affected bounds. High-frequency device events are
resampled through a specified algorithm so document size and output do not depend
on browser timing. Periodic content-addressed checkpoints bound replay time.

## Must ship, in order

- [ ] **1. Freeze the raster-paint specification.** Define pixel formats, tile
  dimensions, sparse storage, canonical stroke samples, interpolation, rounding,
  alpha math, checkpointing, journal compaction, corruption recovery, resource
  limits, and compatibility behavior. Publish JSON Schema plus normative tile,
  stroke, and invalid-document fixtures.

- [ ] **2. Add raster-paint layers and migration.** Create, resize, crop, clear,
  duplicate, merge, and explicitly rasterize layers through shared scene and
  transaction APIs. Integrate tree/search, bounds, groups, masks, clipping,
  effects, components, packages, diff, undo/redo, and multi-page selection.

- [ ] **3. Implement the deterministic brush engine.** Start with hard round,
  soft round, pixel, calligraphic, and textured stamp brushes. Support size,
  hardness, spacing, opacity, flow, angle, roundness, scatter, smoothing, and
  buildup with pinned algorithms. Every stochastic property uses a stored seed.

- [ ] **4. Normalize pen, mouse, and touch input.** Support position, pressure,
  tilt, azimuth, twist, and velocity only where the device reports them. Presets
  map inputs through explicit curves with stable fallbacks. Palm rejection and
  gesture handling remain UI concerns and cannot alter committed stroke math.

- [ ] **5. Add erasing and local blending.** Provide pixel eraser, background
  eraser, clear-to-transparency, smudge, blur, sharpen, dodge, burn, sponge, and
  color-replace tools. Define their sampling radius, accumulation, channel and
  alpha behavior, edge mode, and interaction with selections and masks.

- [ ] **6. Add clone stamp.** Support aligned and non-aligned sampling, explicit
  source point, current/below/specified-layer sampling, transforms, and live
  preview. Stroke records pin the sampled composite or required source revisions
  so later layer changes cannot silently reinterpret an old clone operation.

- [ ] **7. Add healing and patch tools.** Implement deterministic spot healing,
  healing brush, and selection-based patching in Rust with bounded algorithms.
  Separate texture transfer from tone/color matching, expose changed-region
  previews, and version the algorithm. AI cleanup remains an optional v0.9.0
  alternative, never the hidden implementation of a core retouch tool.

- [ ] **8. Add flood fill and contiguous selection.** Implement tolerance,
  connectivity, anti-aliasing, sample scope, gap policy, and transparency handling.
  Bound traversal before allocation and provide progress/cancellation for large
  canvases. Results can fill pixels, create masks, or refine selections.

- [ ] **9. Add pixel selections and transforms.** Provide marquee, lasso, magic
  wand, quick-mask painting, feather, expand, contract, smooth, border, grow,
  similar, invert, and save/load. Selected pixels can move, transform, copy, cut,
  paste, float, or become a new raster layer without losing alpha.

- [ ] **10. Add content-preserving layer operations.** Implement trim, canvas
  resize, rotate/flip, merge visible, merge down, stamp visible, flatten, and
  explicit rasterization. Every destructive-looking action supports dry run,
  revision guards, one transaction, and complete undo until bounded history prunes
  it under documented policy.

- [ ] **11. Add brush presets and assets.** Store presets as readable data with
  versioned engine requirements and content-addressed texture tips. Importing
  third-party brushes is opt-in and converts supported properties explicitly;
  unknown dynamics are reported rather than guessed. Presets contain no scripts.

- [ ] **12. Build editor-grade canvas interaction.** Add cursor outlines, sampled
  previews, stabilizer feedback, source markers, quick mask, selection edges,
  rulers, guides, snapping, navigator, zoom/rotate canvas, and tablet-friendly
  controls. Preview may be approximate while the committed Rust result remains
  authoritative and replaces it promptly.

- [ ] **13. Add CLI and batch parity.** Allow deterministic strokes and retouch
  operations from compact JSON sample streams, preset references, selections, and
  masks. Enforce event, point, tile, surface, and output limits before expensive
  work. Summaries report changed bounds and tile hashes without dumping pixels.

- [ ] **14. Harden performance and correctness.** Benchmark stroke latency, dirty
  tile count, checkpoint cadence, replay, zoom, compositing, memory peaks, package
  size, and undo across large sparse and dense canvases. Fuzz journals, tiles,
  presets, sample streams, selection edges, and malformed brush assets. Run seam,
  alpha, pressure, and replay golden tests on every release target.

## CLI direction

```sh
pentool raster layer add portrait.pen retouch --width 4000 --height 5000
pentool brush preset create soft-retouch --kind soft-round --size 36 --flow 0.12
pentool paint stroke portrait.pen retouch --preset soft-retouch \
  --samples stroke.json --color '#D2A184' --dry-run
pentool clone set-source portrait.pen --layer source --x 820 --y 640
pentool clone stroke portrait.pen retouch --samples clone-stroke.json --aligned
pentool heal spot portrait.pen retouch --x 912 --y 701 --radius 18
pentool select wand portrait.pen --source composite --x 10 --y 10 --tolerance 12
pentool raster merge-down portrait.pen retouch --if-revision <revision>
pentool raster checkpoint portrait.pen retouch --compact
```

Exact command names are provisional. Browser pointer streams use the same canonical
stroke request accepted by CLI and batch. A malformed or interrupted stroke never
leaves a partially committed journal or tile set.

## Acceptance targets

- Retouch a high-resolution portrait with clone, healing, dodge/burn, selections,
  and masks while preserving the original imported asset.
- Paint 10,000 strokes, reopen the document, and reach the current image through a
  bounded checkpoint plus replay budget rather than replaying the entire history.
- Produce identical normative pixels and tile hashes from the same canonical
  strokes on Windows, Linux, Intel macOS, and Apple Silicon.
- Zoom and paint across tile boundaries without visible seams, double application,
  missing pixels, or alpha fringes.
- Cancel a large fill, transform, heal, or stroke without changing document bytes
  or leaving unreferenced committed tiles.
- Package the document, clear disposable caches, move it offline, and reproduce
  the accepted render with every editable raster layer intact.

## Explicitly deferred

- Camera RAW development, ICC-managed wide-gamut editing, CMYK/Lab documents,
  proofing, separations, and print production; these belong to v0.11.0.
- A Photoshop-compatible `.psd` fidelity guarantee. Import/export may be proposed
  later only with a documented mapping and loss report.
- Unbounded procedural brushes, executable brush scripts, and third-party native
  plugins inside the Pentool process.
- Full fluid simulation, neural brushes, face reshaping, puppet rigs, and liquify
  until their deterministic math and bounded interaction model are specified.

## Acceptance demo

Open a layered campaign assembled in v0.8.0, accept a cleaned product image from
v0.9.0, then create raster layers for manual finishing. Remove small defects with
clone and healing, paint edge corrections through a saved mask, dodge and burn the
product, add hand-painted texture with pressure input, and patch one selected
region. Show tile-aware diff, undo/redo, checkpoint compaction, package round trip,
offline reopening, and matching cross-platform pixels without flattening the work.
