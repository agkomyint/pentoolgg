# Local UI verification

2026-10-17, Windows, Chrome, Vite development server on port 5173 with the
local Rust API on port 7878. These checks do not constitute cross-platform CI
or release verification. The embedded production HTML needs regeneration
before packaging; ongoing UI iteration uses Vite hot reload.

## Browser interaction checks

Executed against an isolated browser tab; no Save action or source-file mutation.

- Open the legacy `examples/eye.pen` project and switch through Design, Paint,
  Packaging, Photo, Layout, Images, Composite, Print and Extensions without a
  runtime exception. Only one central stage is visible; the layer tree remains
  visible on desktop. Return to the same Design canvas DOM instance.
- Explicitly upgrade the legacy project through the Paint action, add a raster
  layer on its migrated page, and Undo to restore the legacy format in memory.
- Resize the inspector with the keyboard, collapse it, and reopen it.
- Create a standalone 400 by 300 document, draw a rectangle using mouse events,
  then select Brush from the shared rail.
- Verify Delete in Paint does not invoke the hidden vector canvas's Delete handler.
- Add a raster layer, paint with mouse events, and verify that a pointer at the
  displayed page center sends a stroke sample at layer pixel (200, 150).
- Verify Design and Paint use the same displayed page dimensions. Undo and Redo
  restore the respective document states before and after the stroke.
- Delay a stroke response, edit a vector object while it is processing, and verify
  that the late response preserves the newer document and reports a retry message.
- Check 375, 768 and 1280 pixel viewport widths: one application header and no
  horizontal document overflow. Open and close the mobile inspector overlay.

One local run measured 13–40 ms between changing the workspace selector and two
animation frames completing. This measures shell feedback on a small fixture;
it excludes full preview completion and is not a large-document benchmark.

## Automated checks

- `bun run typecheck`
- `bun test src/lib/model.test.ts src/lib/api.test.ts`: six tests, covering
  transformed raster coordinates, legacy canvas references and drag previews,
  future-version handling, duplicate preview request coalescing, and retry after
  a failed preview.

## Remaining evidence

Large RAW catalogs, large raster documents, populated 3D projects, assistive
technology behavior and cross-platform release artifacts require further checks.
Vector and raster tools retain separate interaction surfaces inside the shared
shell. Resizable panels currently retain their sizes only for the open editor
session. No hosted CI, tag, publication, or release claim is made by these checks.
