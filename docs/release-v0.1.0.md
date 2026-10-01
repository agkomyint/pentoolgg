# Pentool v0.1.0

An agent-friendly vector editor in one Rust binary. Draw with CLI commands,
export without a browser, and edit the same layered artwork visually.

## Included

- Canvas creation, backgrounds, and editable `.pen` JSON documents.
- Layer creation, ordering, visibility, locks, and whole-layer transforms.
- SVG-compatible strokes, fills, Bézier curves, and arcs.
- Shared Rust geometry for translation, rotation, scaling, anchor and handle
  editing, curve splitting, bounds, and hit testing.
- Native PNG and SVG export; no Node.js, browser, or server required for CLI work.
- Embedded browser editor, JSON rendering/geometry API, and explicit shared-file
  save/reload with stale-save detection.
- Editable eye and XYSKID examples and their exported previews.

## Quick start

Download and extract the archive for your operating system, then run:

```sh
pentool serve examples/eye.pen
```

Open http://127.0.0.1:4711. For browser-free output:

```sh
pentool export examples/eye.pen eye.png --scale 2
pentool layer-geometry examples/xyskid.pen --layer symbol translate 40 20
```

On Windows the executable is `pentool.exe`; macOS/Linux users can run `./pentool`
from the extracted folder or add it to PATH.

## Early release limitations

This is a geometry foundation, not a complete replacement for Figma or Photoshop.
Shape boolean operations, automatic snapping/alignment, complete undo history,
and a native desktop window are not included. Browser synchronization is explicit.
Transforms preserve stroke width, and SVG arcs become Bézier approximations when
edited. Binaries are not code-signed.
