# Pentool

Pentool is an open-source, agent-friendly vector editor distributed as **one Rust binary**. It serves a browser canvas, stores editable layered artwork in a readable `.pen` JSON format, exposes rendering APIs, and exports PNG or SVG.

AI agents can draw through CLI commands; humans can edit the same artwork visually.

![Eye drawn with Pentool CLI](examples/eye.png)

Try the bundled examples: `pentool serve examples/eye.pen` or
`pentool serve examples/xyskid.pen`. The eye consists of eight paths on three
artwork layers. Its 15 CLI commands, including two exports, executed in 0.202
seconds on the development machine; this excludes design time and is not a
general performance guarantee.

## Install

Download the binary for your platform from [GitHub Releases](https://github.com/agkomyint/pentoolgg/releases), put it on your `PATH`, then run:

```sh
pentool serve
```

Open `http://127.0.0.1:4711`. No Node.js, database, or external assets are required.

Build from source:

```sh
cargo install --path .
```

## CLI

```sh
pentool serve --host 127.0.0.1 --port 4711
pentool new logo.pen --width 1200 --height 800
pentool info logo.pen
pentool export logo.pen logo.png --scale 2
pentool export logo.pen logo.svg
```

Running `pentool` without a command starts the server.

## Browser-free drawing and shared editing

The CLI creates and edits artwork without a browser or running server. Path data
uses SVG commands: `M` to move, `L` for straight strokes, `C` for cubic Bézier
curves, `Q` for quadratic curves, `A` for arcs, and `Z` to close a shape.

```sh
pentool new artwork.pen --width 800 --height 600
pentool canvas artwork.pen --name "Agent artwork" --background "#ffffff"
pentool layer artwork.pen add ink --name "Ink strokes"
pentool path artwork.pen put curve --layer ink --d "M 80 300 C 160 80 440 80 520 300" --stroke "#2563eb" --width 8
pentool path artwork.pen put triangle --layer ink --d "M 550 100 L 700 350 L 400 350" --closed --fill "#b8f34a"
pentool export artwork.pen artwork.png
pentool export artwork.pen artwork.svg
pentool serve artwork.pen
```

Open `http://127.0.0.1:4711` to view and edit the shared file. **Save shared**
writes browser edits back to that file. **Reload shared** reads the latest CLI
edits. A save is rejected if the file changed since loading; reload before
editing again. Synchronization is explicit, not automatic.

`path put` replaces an existing path with the same ID in that layer. Use
`path remove artwork.pen curve --layer ink` to delete it. Layer commands support
`set ink --name "Details" --visible false --locked true`, `move ink 0`
(back-to-front index), and `remove ink`. Locked layers reject path changes.
`info` prints the complete JSON document for agents to inspect. Existing JSON
extension fields are retained by CLI edits. Run any command with `--help` for
its arguments.

The optional shared-file server exposes `GET /api/document` and
`PUT /api/document` with `{ "base": <loaded document>, "document": <edited document> }`.
Rendering APIs also work independently of a shared document.

## Shared Rust geometry engine

CLI operations and browser Geometry controls use the same `kurbo`-based Rust
engine. It is also exposed as a Rust library (`pentool::geometry`) for future
desktop frontends. No browser is needed for calculations.

```sh
pentool geometry artwork.pen --layer ink --id curve bounds
pentool geometry artwork.pen --layer ink --id curve nodes
pentool geometry artwork.pen --layer ink --id curve hit 200 300 --tolerance 4
pentool geometry artwork.pen --layer ink --id curve translate 20 30
pentool geometry artwork.pen --layer ink --id curve rotate 45 --cx 400 --cy 300
pentool geometry artwork.pen --layer ink --id curve scale 1.5 1.5
pentool geometry artwork.pen --layer ink --id curve move-anchor 0 120 320
pentool geometry artwork.pen --layer ink --id curve set-handle 1 1 180 100
pentool geometry artwork.pen --layer ink --id curve split 0 0.5
pentool layer-geometry artwork.pen --layer ink translate 40 20
```

Layer transforms move every shape, stroke, and fill in that layer together,
preserving stacking order and other layers. Locked layers reject mutations.
Layer changes are calculated completely before applying them. Transforms change
path coordinates; stroke width remains unchanged. Bounds describe geometry,
excluding stroke width. Hit tests use nonzero fill winding and distance to the
stroke centerline plus width/tolerance; they are not pixel-perfect cap/join tests.

`nodes` exposes zero-based anchor indexes and normalized element/handle indexes.
SVG relative commands are normalized; arcs become Bézier approximations when
edited. Re-query nodes after a mutation, especially splitting a segment.

In the browser, select a path and use **Edit anchors / handles** to display
control points and numeric inputs. Drag a displayed anchor/handle to edit it.
Click a path to select it, then drag to move it; set Geometry Target to **Active
layer** to move the whole layer. Move/Rotate/Scale controls also work on either
target. Rotations/scales in the UI use the document origin; CLI commands support
a custom pivot. Geometry edits are undoable with Ctrl+Z / Ctrl+Shift+Z.

`POST /api/geometry` takes:

```json
{"document": {}, "layer": "ink", "id": "curve", "operation": {"type": "translate", "dx": 20, "dy": 30}}
```

Replace `document` with a complete `.pen` document. Omit `id` (or use null) for
a layer transform. The response contains `document` and `result`; computation
does not write the shared file until the browser saves through `/api/document`.

This is a geometry foundation, not yet a full desktop editor: shape boolean
operations, automatic snapping/alignment, complete editing history, and native
desktop window integration remain future work.

## Agent rendering API

The server accepts a complete `.pen` document and returns a rendered file:

```sh
curl -X POST http://127.0.0.1:4711/api/render/png \
  -H "content-type: application/json" \
  --data-binary @logo.pen \
  --output logo.png
```

Endpoints:

- `GET /api/health`
- `POST /api/render/png`
- `POST /api/render/svg`

The API is intentionally simple: an AI agent can create or edit the human-readable path data, preserve layers, and ask the same binary to render it. Canvas dimensions are capped at 16,384 px and request bodies at 16 MiB.

## `.pen` format

`.pen` is versioned JSON containing canvas metadata, ordered layers, and SVG-compatible path data. See [`examples/logo.pen`](examples/logo.pen) and [`docs/pen-format.md`](docs/pen-format.md).

## License

Dual-licensed under MIT or Apache-2.0, at your option.
