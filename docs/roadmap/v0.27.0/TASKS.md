# v0.27.0 — Selection, cutout and crop

> Status: shipped in v0.27.0 (deferred items stay unticked). Checkboxes stay pending until implementation, tests, docs and
> cross-platform evidence exist.

0.27.0 is a focused feature release on selection. The goal is a workflow an agent
(or a person in Studio) can run end to end: look at a photo, select a subject such
as a car, remove the background, crop, and place the cutout on a mood board, all
non-destructively and reproducibly, through the CLI and through Studio.

## Why

Measured on 2026-10-23 with pentool 0.26.0 on a synthetic 640x400 "car on a noisy
gradient sky" image:

- Magic wand tolerance is measured against the seed pixel only. On a gradient sky,
  tolerance 8 selects 11.5k pixels, 16 selects 47k, 32 selects the top half
  (bounds to y=191) and 64 leaks into the car and floods the whole image. No
  tolerance value selects exactly the sky.
- The engine already has `select-modify`, `lift`, `move-pixels`, saved selections
  and `paste`, but the shared dispatcher behind `/api/raster` and Studio only
  accepts marquee, lasso, wand, `select-info` and `select-clear`.
- `pentool selection` (query-based, document level) and `pentool raster select-*`
  (pixel selections on a raster layer) are separate systems that cannot feed each
  other.
- `selection crop` crops the page canvas, not one image; `ai keyout` removes only
  a flat key color; there is no edge-aware selection.
- Studio's canvas marquee selects objects only; image crop is four sliders.

## Non-negotiable boundaries

- One binary, no new dependencies, no network, no model. Algorithms are classical
  and deterministic (same input, same bytes). A learned "select subject" is out of
  scope; agents supply the rough shape (box, polygon or seed points) and the engine
  refines it to image edges.
- Non-destructive by default: cutouts become new nodes or masks; the source image
  is kept. Every mutation is one atomic, undoable transaction with `--dry-run`.
- New work extends the shared raster/selection/mask paths; no parallel model.
- `.pen` changes are additive and optional; version 4-6 documents without them
  behave exactly as before.
- Every new route is classified in `perm::ROUTES`. `web/studio.html` is rebuilt
  only from `web-ui/`.
- Bounded work: every new operation checks pixel/work budgets before running.

## Tasks

### 1. Wand that survives gradients and noise
- [x] `--sample` seed averaging (point, 3x3, 5x5, 11x11) so one noisy pixel does
      not decide the region.
- [x] `--mode-flood gradient` (compare each pixel with its already-selected
      neighbor, bounded step) so skies, studio sweeps and shaded surfaces select
      as one region without leaking into subjects.
- [x] Optional lightness-weighted or Lab distance in addition to RGBA max-channel.
- [x] Tests: the synthetic car/sky fixture (generated in the test, not committed)
      selects the sky with IoU >= 0.98 and never enters the car; existing wand
      tests and goldens unchanged for default arguments.

### 2. Edge-aware refine ("cut the car")
- [x] `select-refine`: take an existing selection or rough shape (box, polygon,
      ellipse), treat outside as background and the inside core as foreground,
      and snap the boundary to image edges with a deterministic min-cut/graph
      region-grow (GrabCut-style, fixed iterations, fixed seed).
- [x] Foreground/background hint strokes (`--fg x,y...` / `--bg x,y...`) to correct
      a result; hints are recorded in the journal.
- [x] `select-modify` additions: `smooth-edge`, `decontaminate` (remove background
      color fringe), `feather` kept; all report selected-pixel counts.
- [x] Tests: synthetic car from a loose box reaches IoU >= 0.95; deterministic
      across runs and platforms; bounded work refusal.

### 3. One-step cutout for agents
- [ ] `pentool raster DOC cutout ID --box|--polygon|--wand|--selection` (mask source pending):
      produces a new transparent raster or masked-image node (source untouched),
      with `--feather`, `--refine`, `--decontaminate`, `--trim`, `--id`.
- [ ] `--dry-run` and `--preview` report selected area, bounds and edge confidence.
- [x] Result JSON is compact: new node ID, bounds, selection hash, warnings.
- [x] Works on image nodes without a manual `rasterize` step (done internally).
- [x] Mood-board acceptance test: import 3 images, cutout each, place and export.

### 4. Bridge the two selection systems
- [x] Query kinds usable as raster selections (`color-range`, `luminosity-range`,
      `node-alpha`, `mask-alpha`, polygon) and raster selections usable as mask
      sources and `selection` query kinds (`raster-selection`, `saved-selection`).
- [x] `selection crop --node ID` crops one image or group to the selection bounds
      (no page resize); the page-crop behavior stays the default and unchanged.
- [x] Polygon limit raised or replaced by a simplified-path form; add invert,
      grow/shrink and feather to query selections.

### 5. Shared dispatcher and API
- [x] Expose `select-modify`, `select-save/load/delete`, `lift`, `move-pixels`,
      `transform-pixels`, `paste` and the new actions through the shared raster
      dispatcher, so `/api/raster`, batch and Studio match the CLI.
- [x] `perm::ROUTES` classification and `server::tests` coverage.

### 6. Studio selection tools
- [x] A selection toolbar with replace/add/subtract/intersect, feather, expand,
      contract, invert, smooth, refine edge, and clear.
- [ ] Marching-ants outline, move/transform the selection outline, quick mask,
      nudge by keyboard, Shift/Alt modifiers matching familiar editors.
- [x] Wand options: tolerance, sample size, contiguous, gradient mode, anti-alias.
- [x] Canvas crop tool for image nodes with handles and aspect presets.
- [ ] Cut to layer, Copy to layer, Delete background (remove selection), Mask
      from selection, with undo as one step. (Mask from selection deferred.)
- [x] `bun run typecheck`, `bun test` and embed checks pass; no hand edits of
      `web/studio.html`.

### 7. Docs, schema and agent guide
- [x] `docs/` section "Selecting and cutting out" with the agent recipe (view,
      outline, refine, cutout, place) and CLI help examples that run.
- [x] README and `--help` synchronized; `docs/release-v0.27.0.md`.

## Acceptance

- The synthetic car fixture: wand selects sky only; refine-from-box selects the
  car with IoU >= 0.95; the cutout has clean alpha with no sky fringe.
- An agent can complete a three-image mood board using only CLI commands.
- Studio performs the same flow, and undo returns the exact previous bytes.
- All earlier raster, mask, selection and golden tests pass unchanged.

## Explicitly deferred

- Learned subject, sky or object detection (needs a model or new dependency).
- Video/animation masks.
- GPU acceleration of selection.
