# `.pen` file format, versions 1 and 2

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
