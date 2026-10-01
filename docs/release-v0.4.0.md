# Pentool v0.4.0 — pages and `.pen` composition

Pentool v0.4.0 turns `.pen` into a multi-page, composable design format while
keeping every operation available without a browser.

## Highlights

- Format v3 adds ordered pages with independent canvases and layer stacks.
- v1/v2 files remain readable as `page-1` and upgrade without visual changes.
- Page-aware CLI editing, discovery, geometry, export, and browser controls.
- Transactional `.pen` import with dry runs, revision guards, backups, placement
  transforms, deterministic ID prefixes, font deduplication, and canvas expansion.
- Unknown source extension fields survive imports.
- Search supports compact pagination with `--offset` and `--limit`.
- Indexed layer/object lookup improves targeted agent operations.
- A repeatable `benchmark` command covers parsing, validation, indexing, search,
  one-object editing, SVG generation, and optional PNG rendering.
- Browser object results are debounced and bounded, and `.pen` files can be
  imported into the active page through the same Rust engine used by the CLI.

## Examples

```sh
pentool page design.pen add mobile --name Mobile --width 390 --height 844
pentool --page mobile import design.pen icons.pen --prefix icons --at 100 80 --dry-run
pentool --page mobile search design.pen arrow --offset 0 --limit 25
pentool --page mobile export design.pen mobile.png
pentool benchmark --layers 1000 --objects 100000
```

Imports copy editable content; they are not live links. Every committed import
creates a numbered recovery snapshot beside the destination.
