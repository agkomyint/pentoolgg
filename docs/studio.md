# Pentool Studio (web-ui)

`web-ui/` is the TypeScript source of the new browser UI: Vite, React 18,
Tailwind CSS 4 and shadcn-style components (Radix primitives, `cva`), with
Zustand for state. It is served at `/` (and `/studio`) and is the only browser editor; the earlier
vanilla-JS editor (`/classic`) was retired once its photo and raster panels were ported.

## Product properties

- `cargo build` never needs Node or Bun. `bun run build` produces one
  self-contained `web/studio.html`, which is committed and embedded in the
  binary (`include_str!`). Rebuild and commit it whenever `web-ui/src` changes.
- The server stays the source of truth. Studio renders previews through
  `/api/render/png`, runs checks through `/api/color/*`, and never reimplements
  document logic in the browser.
- Workspaces map to the roadmap (`web-ui/src/lib/workspaces.ts`): Design,
  Images (0.7), Composite (0.8), Photo (0.11), Print (0.12), Paint (0.13),
  Layout (0.14). A workspace whose format or CLI is
  not implemented says so and lists what the roadmap promises; it has no
  controls that cannot work.

## Develop

```sh
pentool serve artwork.pen --port 7878      # terminal 1
cd web-ui && bun install && bun run dev    # terminal 2, proxies /api to :7878
bun run build                              # typecheck + refresh web/studio.html
```

## Status

Editing (document JSON edited in the browser, validated by Rust on render and
save, one undo step per edit): select/move/resize, pen (bezier, close, finish),
rectangle, ellipse, line, text, hand/pan/zoom, delete/duplicate/group/ungroup,
z-order, layers (visibility, lock, rename, add, delete), pages, canvas size and
background, paste/drop images (`/api/image/add`), embed fonts, import `.pen`,
asset library search, property inspector (geometry, fill, stroke, radii, text),
raw node JSON, a scene-batch console, Images (operations stack), Composite
(blend, opacity, adjustments, masks, clips; needs format v6), Print (preflight,
plates, soft proof), New/Open/Save (shared file or `.pen` download), PNG/SVG
export, dark mode.

Photo (catalog grid, loupe, compare, culling keys, histogram and clipping overlays, develop sliders,
white balance, crop, radial/linear/brush masks, copy/sync, variants, snapshots, keywords) and Paint
(raster brush, eraser, clone, heal, quick mask, marquee/lasso/wand selections, rulers, guides, snap,
navigator) workspaces use the same `/api/photo/*` and `/api/raster` endpoints as before.
Known limits: hit-testing is bounding-box based; during a drag only the
selection box moves and the rendered preview updates on release; there is no
in-browser PDF export (use `pentool export x.pen x.pdf`); Layout is a
"coming soon" placeholder and Paint has no agent-directed painting yet (0.13).
