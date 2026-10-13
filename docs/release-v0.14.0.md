# Pentool v0.14.0

Professional page layout. Documents with layout use `.pen` format v9; earlier
versions keep working and migrate explicitly. Existing CLI flags and JSON fields
keep their behavior.

## Layout engine

- Spreads, parents, sections and page numbering; stories with threaded frames;
  a shaping engine and paragraph composer; paragraph, character, object, table
  and cell styles with regex styles; baseline grids, guides and alignment.
- Text wrap and anchored objects; tables with merged cells and repeating
  header rows; TOC, index, cross-references, links and bookmarks.
- `.penbook` books with continuous numbering, synchronization, book-wide TOC
  and index, preflight and packaging; data merge and adaptive layout.

## Interchange and output

- IDML import and export (features without an exact equivalent fail the export
  unless `--allow-approximation`); tagged PDF and EPUB 3 (reflowable and fixed);
  layout preflight.

## Studio

- Layout workspace: pages panel, story editor, frame threading, tables,
  wrap/anchor controls, styles of all five kinds with property editing, TOC,
  index and links, preflight, overlays (frames, threads, overset, margins,
  columns, baseline grid, guides, hidden characters), loaded-cursor placement of
  text files, and an opt-in InDesign key set. Every edit is one `POST /api/layout`
  op through the Rust planners and one undo step.
- Local WASM preview core and a queued, non-blocking paint pipeline with
  immediate stroke feedback.

## Fixes

- Pages whose canvas size is stored as a float (as IDML import writes it) now
  render and export.
- JSON table actions use the same defaults as the CLI.

## Known limits

- Recomposition is whole-document (616 ms cold on 500 pages, ~20 ms cached);
  per-story-range incremental recomposition is not implemented.
- Loaded-cursor placement of images and on-canvas anchor handles are not built.
- Golden composition on all four targets, fuzz targets for IDML/EPUB/tables,
  cancellation at every import/compose/export stage and book crash-recovery
  tests are not complete (see `docs/roadmap/v0.14.0/TASKS.md`).
