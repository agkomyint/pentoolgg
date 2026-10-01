# `.pen` file format, versions 1–3

A `.pen` file is UTF-8 JSON. The format is deliberately inspectable and safe for humans, scripts, and generative agents to edit.

```json
{
  "format": "pentool",
  "version": 1,
  "name": "Example",
  "canvas": { "width": 1200, "height": 800, "background": "#ffffff" },
  "layers": [{
    "id": "layer-1",
    "name": "Artwork",
    "visible": true,
    "locked": false,
    "paths": [{
      "id": "path-1",
      "d": "M 100 100 C 200 20 300 180 400 100",
      "stroke": "#111827",
      "stroke_width": 8,
      "fill": "none",
      "closed": false
    }]
  }]
}
```

Path `d` uses standard SVG path syntax. Layer order is back-to-front; path order inside a layer is also back-to-front. Unknown top-level fields should be preserved by tools that edit documents. Version 1 supports solid CSS colors. External resources, scripts, filters, and raw SVG markup are not part of the format.

Limits enforced by the renderer:

- canvas: 1–16,384 px on each axis
- layers: at most 1,000
- paths: at most 100,000 total
- HTTP request: at most 16 MiB

## Version 2: editable text and embedded fonts

Pentoolgg v0.3.0 keeps file format version 2 unchanged. Agent discovery and batch
editing are tool behaviors, not a schema migration. Within every layer the
explicit render order is the layer's `paths` array followed by its `texts` array;
zero is the back of each stack. Use separate layers when paths and text must be
interleaved. Unknown fields are retained by supported CLI edit operations.

Version 2 paths also support `stroke_linecap` (`butt`, `round`, `square`),
`stroke_linejoin` (`miter`, `round`, `bevel`), and `stroke_miterlimit` (1–1000,
default 4). Omitted caps/joins default to round to preserve older artwork.
New paths made through the CLI/browser default to butt/miter. Native rendering
and SVG export use these properties directly. Styling upgrades v1 files to v2.

Version 1 path-only documents remain readable. New documents use version 2;
adding text or fonts upgrades older documents. Older v0.1 binaries reject version
2 instead of silently removing typography.

Each layer can have a `texts` array. Paths draw first, then text objects, both
back-to-front within their arrays. Use separate layers to interleave paths and
text. IDs must be unique across paths and text within the same layer.

```json
{
  "id": "title",
  "content": "Editable text\nSecond line",
  "x": 100,
  "y": 200,
  "font_family": "Atkinson Hyperlegible",
  "font_size": 48,
  "font_weight": 400,
  "italic": false,
  "fill": "#111827",
  "align": "left",
  "letter_spacing": 0,
  "line_height": 1.2,
  "transform": [1, 0, 0, 1, 0, 0]
}
```

Positions reference the first line's baseline. Newlines are explicit; wrapping
is not automatic. `align` is `left`, `center`, or `right`, relative to `x`.
Size and spacing use canvas units; leading is a font-size multiplier. The six
transform values follow SVG matrix order `[a,b,c,d,e,f]`. Transform operations
compose this matrix without changing content or typography. Identity matrices
may be omitted. Locked layers protect both text and paths.

Top-level `fonts` is an optional array of `{ "id": "brand", "data": "<base64>" }`
resources containing valid TTF/OTF data. Family/style metadata is read from the
font. Embedded fonts are prioritized over bundled and installed faces. The
bundled default is not duplicated in `.pen` files. Font embedding is not a
license grant: only distribute fonts whose license permits it.

Additional limits: 100,000 text objects; one million UTF-8 bytes per content;
font size 0.1–4,096; weight 100–900; leading 0.1–10; at most 64 embedded fonts;
8 MiB base64 per font and 16 MiB total base64 font data. Geometry/typography
numbers must be finite and text must contain XML-safe characters. Documents
near these limits may exceed the HTTP body limit once JSON overhead is included.

## Version 3: pages

Version 3 replaces the single top-level `canvas` and `layers` fields with an
ordered `pages` array. Each page owns its canvas and ordered layers. Fonts remain
document-wide so pages can share an embedded face without duplicating its bytes.

```json
{
  "format": "pentool",
  "version": 3,
  "name": "Product",
  "pages": [{
    "id": "desktop",
    "name": "Desktop",
    "canvas": { "width": 1440, "height": 1024, "background": "#ffffff" },
    "layers": []
  }],
  "fonts": []
}
```

Page IDs are unique and stable. Page order is presentation order and does not
affect rendering. A document contains 1–1,000 pages; each page contains at least
one layer and retains the existing per-page layer limits. The path and text limits
apply across the complete document.

Pentoolgg continues to read v1/v2 documents as a virtual `page-1`. Adding a page
upgrades the document to v3 without changing the original canvas or artwork.
Commands that operate on canvas content accept global `--page <id>` selection.

## Import and composition rules

v0.4 imports copy source layers into one destination page. A prefix is applied to
layer and object IDs by default; disabling it makes any collision an error. Source
layer order and object order are retained. Placement scale, rotation, and
translation are applied to editable geometry and text transforms rather than
flattening the result. The destination canvas remains unchanged unless
`--expand-canvas` is supplied.

Embedded fonts are document-wide. Byte-identical font data is reused; a different
font whose resource ID collides receives a deterministic numbered ID. Unknown
JSON fields on imported layers and objects are preserved. Imports are copies, not
live links to the source file.
