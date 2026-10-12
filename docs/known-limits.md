# Known limits and record shapes

This page records limits that are deliberate (or not yet lifted) and the shape of
records that are easy to get wrong. It is a statement of current behavior, not a
roadmap; check the code and tests before relying on an unlisted capability.

## Spot-ink library record

A spot-ink library is a JSON object with `schema` (currently `1`) and an `inks`
map keyed by the ink's stable ID. Unknown fields are rejected.

```json
{
  "schema": 1,
  "inks": {
    "brand-orange": {
      "id": "brand-orange",
      "name": "Brand Orange",
      "aliases": ["orange-1"],
      "alternate": {"space": "cmyk", "...": "native color value"},
      "provenance": {"source": "internal brand guide", "publisher": "optional",
                     "revision": "optional", "license": "optional"},
      "redistribution": "unknown"
    }
  }
}
```

- `id` and every alias match `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`; aliases are
  case-insensitive, unique per ink and may not repeat the ID. At most 64 aliases
  per ink, 512 inks and 2048 aliases per library, 4 MiB per library file.
- `name` is a nonempty display string of at most 256 UTF-8 bytes without control
  characters. Names and aliases are never identity; the `id` is.
- `alternate` is a supported native color that is **not** itself a spot ink. It is
  used for preview only and is never a plate substitute.
- `provenance.source` is required and descriptive; Pentool never fetches it. The
  optional fields share the 4096-byte limit.
- `redistribution` is `allowed`, `restricted` or `unknown`; it is never inferred.
- A paint that uses an ink is `{"space":"spot","ink":ID,"tint":0-1,"alpha":0-1}`.

## Raster painting (agent painting and fields)

- A field accepts at most 512 strokes per pass (`MAX_FIELD_STROKES`) and 512
  samples per stroke (`MAX_STROKE_SAMPLES`). Split larger plans across passes.
- Field cell sizes are limited to 1-512 pixels.

## Argument shapes

- `color production set --geometry JSON` **replaces** the page's production object
  and reports the keys it dropped under `dropped_keys`. Pass `--merge` to apply an
  RFC 7386 merge-patch instead (nested objects merge, `null` removes a key).
- `canvas`, and every command that goes through the shared page-edit path, reports
  `changed` and, when nothing changed, a `note` so a no-op is distinguishable from
  an applied edit.
- `mask create` stores a named resource. Masks are not scene nodes, so they do not
  appear in `tree` or `search`; use `mask list` and `mask attach`.
- `raster add` keeps the document's version (v6 or later); only older documents are
  migrated to v6.
