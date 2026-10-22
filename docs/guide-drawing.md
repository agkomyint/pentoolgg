# Drawing, text, and pages

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [Multiple pages](#multiple-pages)
- [Browser-free drawing and shared editing](#browser-free-drawing-and-shared-editing)
- [Stroke edges](#stroke-edges)
- [Editable text and fonts](#editable-text-and-fonts)
- [Shared Rust geometry engine](#shared-rust-geometry-engine)
- [`.pen` format](#pen-format)

## Multiple pages

Version 3 `.pen` documents contain ordered pages, each with its own canvas and
layers. Existing v1/v2 files still open as `page-1`; adding another page upgrades
them without changing their original artwork.

```sh
pentool page design.pen list
pentool page design.pen add mobile --name "Mobile" --width 390 --height 844
pentool page design.pen duplicate mobile mobile-alt
pentool page design.pen move mobile-alt 0
pentool page design.pen rename mobile-alt "Mobile alternative"
pentool --page mobile search design.pen button
pentool --page mobile export design.pen mobile.png
```

The browser page panel switches canvases, adds, renames, and removes pages. All
editing and rendering API requests carry the selected page ID so an operation
cannot accidentally target a similarly named layer on another page.

### Compose `.pen` projects

Import another project into the selected page without flattening its paths or
text. Imports validate first and write atomically; the original destination is
retained in bounded hidden history for recovery.

```sh
pentool --page mobile import design.pen icons.pen \
  --prefix icons --at 120 80 --scale 0.75 --rotate 5 --expand-canvas --dry-run
pentool --page mobile import design.pen icons.pen \
  --prefix icons --at 120 80 --revision 0123456789abcdef
```

Omit `--prefix` to derive one from the source filename, or pass `--no-prefix` to
require collision-free source IDs. Byte-identical fonts are deduplicated. Import
copies content; it does not create a live dependency.

### Import static design formats

The design interchange importer accepts bounded static SVG, PDF with classic
xref tables or plain/FlateDecode xref streams, including bounded type-2 entries
for non-stream values in plain/FlateDecode object streams,
(including PDF-compatible Illustrator `.ai` files), bounded PSD layer subsets,
Sketch archives, saved Figma REST `DOCUMENT` JSON, and legacy unwrapped Figma
`.fig` v15 subsets. `.ai` export uses the PDF writer and is not a native
Illustrator export. Imports into v4+ `.pen`
documents are placed in a separate namespaced layer and return a fidelity report
(`exact`, `approximated`, and `dropped` counts). Save the detailed report with
`--report`; unsupported features are reported or rejected rather than silently
executed. Sketch symbols/shared styles and Figma responsive auto-layout are not
fully preserved yet. SVG local `<use>` references to primitive shapes and static
groups and viewBox-backed symbols expand into editable geometry with bounded
instance transforms; unsupported symbol content remains reported as dropped.
Sketch maps shared left/center/right text alignment and
applies uniquely matched string overrides to materialized and master-derived
text fallbacks; other symbol overrides remain unsupported. Figma component definitions and instance links are retained
as Pentool component snapshots with visual fallbacks. Unique `TEXT` component
properties named `<layer>#text` are applied to the matching materialized
fallback text node; other property types, ambiguous matches, and live override
propagation remain unsupported. PDF import supports classic cross-reference tables, plain/FlateDecode xref streams, and bounded object-stream scalar/array/dictionary entries, vector paths,
simple one-byte `Tj`/`TJ` text runs, permitted embedded TrueType fonts, FlateDecode
RGB/gray and packed 1-bit gray images, FlateDecode DeviceCMYK images, and
baseline JPEG DeviceRGB/DeviceGray images, plus matching 8-bit DeviceGray
FlateDecode image soft masks applied as alpha. DeviceCMYK vectors and images use
a reported simple sRGB approximation. 8-bit DeviceGray/DeviceRGB color-key
`/Mask` arrays are applied as binary transparency. Calibrated colors, other
soft-mask variants, object-stream members containing streams, nested object
streams, inline images, and incremental/hybrid xrefs remain unsupported. PSD import supports raster layers/folders, root-level clipping
alpha against the preceding base layer, and bounded 8-bit raster masks with
relative-position and/or invert flags; adjustment/effect layers and other mask layouts are
unsupported. PSD export writes a flattened composite or named full-canvas raster
layers with visibility preserved; editable vector structure and profiles are
not retained. Figma import accepts saved REST JSON and the
legacy unwrapped `fig-kiwi` v15 Deflate container. ZIP/Zstandard containers,
FigJam files, and other `.fig` versions are rejected; see the
[interchange limits](../docs/design-interchange-v1.md#native-figma-fig-subset).

```sh
pentool import artwork.pen icons.svg --report icons-fidelity.json
pentool import artwork.pen proof.pdf --report proof-fidelity.json
pentool import artwork.pen layered.psd --report layered-fidelity.json
pentool import artwork.pen library.sketch --prefix library
pentool import artwork.pen figma-export.json --format figma
pentool import artwork.pen design.fig --report fig-fidelity.json
```

Studio also exposes SVG, PDF, PSD, Sketch, Figma JSON, and supported `.fig` import in the Design panel and
shows the returned fidelity report. This does not change the existing legacy
`.pen` composition workflow above.

Exports to PNG, SVG, PDF, and PSD emit bounded fidelity counts in the command's JSON
result. PSD export defaults to one flattened 8-bit RGBA page. Add `--layers` to
write one full-canvas raster layer per source layer, preserving layer names and
visibility. Neither PSD mode retains vector editability or profiles. Pass
`--report PATH` to save the full per-object report.

```sh
pentool export design.pen flattened.psd --report psd-fidelity.json
pentool export design.pen layered.psd --layers
```

Large searches are paginated for compact agent context:

```sh
pentool --page mobile search design.pen icon --offset 100 --limit 50
pentool benchmark --layers 1000 --objects 100000
pentool benchmark --render --layers 100 --objects 10000 --warmups 2 --repetitions 7 --scale 1 --json
```

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

Open `http://127.0.0.1:4711` to view and edit the shared file. **Save**
writes browser edits back to that file. **Reload project** reads the latest CLI
edits. A save is rejected if the file changed since loading; reload before
editing again. Synchronization is explicit, not automatic.

`path put` replaces an existing path with the same ID in that layer. Use
`path remove artwork.pen curve --layer ink` to delete it. Layer commands support
`set ink --name "Details" --visible false --locked true`, `move ink 0`
(back-to-front index), and `remove ink`. Locked layers reject path changes.
`info` prints document metadata for agents to inspect. Existing JSON
extension fields are retained by CLI edits. Run any command with `--help` for
its arguments.

The optional shared-file server exposes `GET /api/document` (document and opaque
revision token) and `PUT /api/document` with
`{ "revision": "<loaded token>", "document": <edited document> }`.
Retain the token from loading or the last successful save. Legacy `base` requests
remain supported, but revision tokens avoid browser number-conversion conflicts.
Rendering APIs also work independently of a shared document.

## Stroke edges

New CLI/browser paths use flat caps and sharp corners by default. Existing paths
without edge properties retain their original round styling. Choose end caps
(`butt`, `round`, `square`), corner joins (`miter`, `round`, `bevel`), and a miter
limit in the browser Style inspector, JSON, or CLI:

```sh
pentool path artwork.pen put angle --layer ink --d "M 100 300 L 200 100 L 300 300" --stroke "#b8f34a" --width 20 --cap butt --join miter --miter-limit 8
pentool path artwork.pen style angle --layer ink --cap square --join bevel
```

`path style` preserves geometry and colors. Miter joins form sharp stroke points;
corners exceeding the miter limit fall back to beveling. Limits are finite values
from 1 to 1000. The same settings drive browser previews, native PNG, and SVG.
Caps/joins affect strokes, not filled silhouettes. Sharp versus smooth curve
anchors are controlled separately by their Bézier handles. Stroke hit tests
remain centerline-based approximations, not exact cap/join hit tests.

![Round, sharp, and beveled stroke edges](../examples/stroke-edges.png)

Open the editable comparison with `pentool serve examples/stroke-edges.pen`.

## Editable text and fonts

Text is stored as content and typography, not hand-drawn strokes. The Rust CLI
creates, measures, transforms, and renders it without opening a browser.

```sh
pentool new typography.pen --width 1200 --height 720
pentool text typography.pen put title --layer layer-1 --content "pentoolgg" --x 100 --y 240 --size 120 --weight 700
pentool text typography.pen set title --layer layer-1 --content "Still editable" --italic true
pentool geometry typography.pen --layer layer-1 --id title bounds
pentool layer-geometry typography.pen --layer layer-1 translate 30 20
pentool export typography.pen typography.png --scale 2
pentool export typography.pen typography.svg
pentool export typography.pen typography-outlined.svg --outline-text
pentool fonts
pentool font typography.pen add brand-font --file ./brand-font.ttf
pentool text typography.pen set title --layer layer-1 --font "Your Font Family"
pentool serve typography.pen
```

Use `text put` to create/replace, `text set` to change only supplied properties,
and `text remove` to delete. Options include family, size, weight, italic, fill,
left/center/right alignment, letter spacing, and line-height multiplier.
`x`/`y` position the first line's baseline; explicit newlines create more lines.
Whole-layer transforms move paths and text together; text remains editable.
Text hit testing uses layout bounds, not pixel-perfect glyph outlines.

In the browser, choose **Text (T)**, enter content/style in the inspector, and
click the canvas. Use **Select (V)** to edit or drag text. **Embed TTF / OTF font**
stores a font in the document; normal undo and shared saving work with text.

Atkinson Hyperlegible regular, bold, italic, and bold italic are bundled in the
binary, so the default needs no installed fonts or network. Other requested
weights use the closest available font face. Installed fonts are also available
for native rendering, but a remote browser might not have them: embed custom
TTF/OTF files you are licensed to redistribute for portable artwork. Family
names come from font metadata, not filenames. Generic family aliases resolve
to the bundled default in native rendering; use explicit family names for
consistent browser previews.

Live SVG exports contain `<text>` plus bundled/embedded font data. Some SVG
consumers do not support embedded fonts; `--outline-text` exports glyph paths
instead, while the original `.pen` keeps editable text. PNG uses native font
shaping and rasterization. System-only fonts are not embedded automatically.
For wrapped text and later typography features, see the [layout reference](layout-v1.md)
and `pentool text-box --help`.

![Editable typography demo](../examples/text-demo.png)

Try `pentool serve examples/text-demo.pen`. [Live SVG](../examples/text-demo.svg) ·
[Outlined SVG](../examples/text-demo-outlined.svg) · [Editable document](../examples/text-demo.pen)

The API adds `POST /api/text` with `{ "document": {}, "action":
{ "type": "put", "id": "title", "layer": "layer-1", "content": "Hello" } }`.
Use `type: "set"` or `"remove"` for updates/deletion. `GET /api/fonts` lists faces;
`POST /api/font` accepts `{ "document": {}, "id": "brand", "data": "<base64 TTF/OTF>" }`.
These operations return the updated document without writing the shared file.

### Rust dependencies

`resvg`/`usvg` provide SVG rendering, font discovery/parsing (`fontdb`), and
glyph shaping (`rustybuzz`); `tiny-skia` rasterizes PNG; `kurbo` handles vector
geometry. `serde`/`serde_json` serialize documents, `base64` packages font data,
`clap` supplies the CLI, and `axum`/`tokio` supply the optional HTTP server.
No JavaScript runtime is required to use or build the native renderer.

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

See `pentool geometry --help` for the current operation list and the
[Studio guide](studio.md) for browser editing controls.

## `.pen` format

`.pen` is versioned JSON containing canvas metadata, ordered layers, and SVG-compatible path data. See [`examples/logo.pen`](../examples/logo.pen) and [`docs/pen-format.md`](../docs/pen-format.md).
