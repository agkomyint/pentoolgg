# Pentool v0.28.0

Studio selections now show their real shape and can be painted, photos can be edited
in place, and two new agent tools arrive: `pentool watch` and `pentool report`.
The library is also split into a `pentool-core` crate that compiles to WebAssembly.
No new dependencies, no model, no network; `.pen` files and existing flags and
JSON fields are unchanged (new fields are additive).

## New

- **`pentool watch DOC`:** one JSON line per change with the actor and the ids of
  added, changed and removed objects, for agents that react to each other's edits.
- **`pentool report`:** build cited, charted reports from a JSON spec and CSV, TSV
  or JSON data files (bar, column, segmented, table, stat tiles, callouts), with
  numbered citations, a generated Sources page, auto-flow pagination and
  `check --strict`. See [Reports](reports.md).
- **Studio selection you can see:** every selection result carries a bounded
  `preview` coverage grid (at most 256 cells on the long edge). Studio tints the
  real shape and draws its true outline instead of the bounding box.
- **Studio selection brush (A):** paints into the selection with the shared
  quickmask engine action; Alt or subtract mode removes.
- **Studio Move selection tool:** drags the selected pixels; Alt copies.
- **Studio edits photo pixels:** selecting an image and switching to a paint or
  selection tool (or pressing Edit photo pixels) rasterizes it in place, so
  marquee, wand, brush and Move selection work on photos.
- **Studio shortcuts:** Ctrl+C and Ctrl+J copy the selection to a layer, Ctrl+X cuts
  it, Ctrl+Shift+I inverts it, and Delete erases the selected pixels.

## Fixed

- The rasterizer clamps over-alpha pixels, so a premultiplied pixel can no longer
  exceed its alpha.

## Internal

- **`pentool-core` crate:** the document and scene model, renderer, exporters,
  transactions, libraries and the collaboration op-log now live in
  `crates/pentool-core`, which builds for `wasm32-unknown-unknown`. The `pentool`
  crate keeps the CLI, server, AI, extensions and networked collaboration, and
  re-exports every core module under its old path. Output bytes are unchanged.
  See [Architecture](architecture.md).

## Not included

- A `cutout` mask source and `--preview`, a non-destructive mask from a selection,
  moving, transforming or nudging the selection outline in Studio, and a
  live-browser check of the selection brush stroke preview. These stay open in
  `docs/roadmap/v0.28.0/TASKS.md`.

## Compatibility

Existing flags, JSON fields and `.pen` files behave as before. The selection
`preview` field is additive. Rust code that depends on the `pentool` library keeps
its module paths.
