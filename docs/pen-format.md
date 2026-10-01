# `.pen` file format, version 1

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

