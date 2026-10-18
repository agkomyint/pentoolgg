# Pentool v0.20.1 — Stabilization patch

Bug fixes and a Studio usability pass. No `.pen` format, CLI, or API shape changes.

## Studio

- **One canvas.** Painting happens directly on the design canvas; the separate
  paint stage is gone. A panel rail on the right switches the dock between
  Properties, Insert and assets, Images, Composite, Paint, Layout, Print, Design
  systems, Collaboration, Stats, and Extensions. Only Photo and Packaging and 3D
  open a focus view, with a "Back to canvas" bar.
- **Page strip.** Pages are managed in one place under the canvas: thumbnails,
  "+" to add after the current page, drag to reorder, F2 or double-click to
  rename, Delete (undoable), Alt+Left/Right, and a right-click menu. Unnamed
  pages read "Page N". Thumbnails that fail while preview workers are busy retry.
- **Sharp zoom.** Previews are requested at the displayed size times the device
  pixel ratio, so zoomed artwork stays sharp; the preview routes' `max_edge`
  range widens from 1..4096 to 1..8192 (bounded by 8x the canvas and about 50
  million pixels). Preview sizing never changes exported pixels. Ctrl+wheel zooms at
  the pointer; Ctrl+0 fits, Ctrl+1 is 100%. Large composite previews step down in
  scale instead of failing on the preview budget.
- **Raster tools.** The in-progress stroke preview is clipped to the active
  selection, as the committed stroke already was. Ctrl+D deselects in paint mode
  instead of duplicating. Every paint tool reuses one paint layer per page.
  Information messages fade; errors stay.
- **EXT-001.** Collaboration, Stats, and Design systems are reachable (rail panels).

## Fixes

- **CORE-001.** `pentool add` into a v4+ document (the `pentool new` default) now
  fails with `[unsupported-version]` and names the component workflow
  (`group promote` / `group instantiate --component`) or `migrate --target 3`,
  instead of "document has no canvas". README documents the split.
- **RASTER-001.** Docs and the gradient-overlay hint said gradient stops use
  `offset`; the released key is `position`. Docs are corrected and a stop with
  `offset` now gets an error naming `position` (fill, gradient-map,
  gradient-overlay).
- **X3D-001.** The shipped `examples/v015-glass-crystal` mockup exports again
  (`supersample: 2`, inside the unchanged 256 MiB render budget).
- **X3D-002.** Packaging, binding, and batch commands accept format v12 (any
  format v11 or newer) instead of only v11.
- **X3D-006.** CLI import of an SVG with a clip mask merges its mask resources
  (deduplicated, mask cap enforced) and validates.
- **X3D-007.** `tree` and `search` work on imported `.fig` documents: a node with
  no drawable geometry reports zero-size bounds at its origin.
- **EXT-002.** With a collab token, a comment's author comes from the token, not
  the request body.
