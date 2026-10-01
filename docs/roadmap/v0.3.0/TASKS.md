# v0.3.0 — Easier editing for agents and humans

Prioritize reliable CLI discovery and targeted edits before adding more canvas
features. Pages/artboards are useful, but should not delay the core editing work.
This is a proposed checklist, not functionality available in v0.2.0.

## Must ship, in order

- [x] **1. Find layers and objects.** Add compact JSON list/tree/search commands
  filtering by ID, name, object type, layer, and text content. Return stable IDs,
  bounds, visibility, and lock state; report ambiguous matches instead of guessing.
- [x] **2. Edit without replacing.** Add partial path/property updates and object
  rename, duplicate, move-to-layer, and reorder. Preserve geometry, unspecified
  styles, text settings, and extension fields. Make path/text stacking explicit.
- [x] **3. Safe batch CLI edits.** Accept multiple operations in one JSON request;
  validate and apply all-or-nothing. Include dry-run/change summaries, expected
  revision checks, atomic file writes, and an undo/recovery snapshot.
- [x] **4. Better browser layer panel.** Show individual paths/text under each
  layer, with search, rename, reorder, duplicate, delete, and lock/hide controls.
  Selecting a result must highlight the correct canvas object. Reuse Rust editing
  operations rather than maintaining different browser behavior.
- [x] **5. Tests, docs, and release checks.** Test CLI/browser parity, stale edits,
  locked layers, batch rollback, stacking, and old-file compatibility. Document
  agent workflows; run existing rendering tests on all four release platforms.

## Stretch goal

- [ ] **6. Basic pages/artboards (deferred).** Each page owns a canvas and layers. Support
  create/list/rename/select and per-page export in CLI and browser. Specify format
  migration first: old files become one page without losing data. Defer to v0.4.0
  if this puts the must-ship tasks at risk; full nested frames/groups come later.

## Acceptance demo

An agent can find “composer,” change only its color, duplicate it, move the copy,
and export—without parsing the entire document or overwriting a human's edits.
A human can find and select the same objects visually. All existing examples
continue to render correctly.
