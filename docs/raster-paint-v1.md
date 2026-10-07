# Raster-paint layers (v0.10, engine 1)

A raster layer is a v6 node of `kind: "raster"` holding editable pixels. It is the
only node that stores pixels in the `.pen` itself; imported photos stay `image` nodes.

## Storage

- Surface: `width` x `height` pixels (1-16384), `pixel_format: "rgba8-straight"`.
- Pixels live in sparse 256x256 tiles. `tiles` maps `"x,y"` (canonical decimal, no
  leading zeros) to a `sha256:` digest; absent tiles are fully transparent. All-zero
  tiles are never stored. At most 4096 tiles per layer.
- The document's top-level `raster_tiles` maps digest -> `{encoding:"png-base64",data}`.
  Identical tiles share one entry; unreferenced entries are removed on every mutation.
  The digest is over the raw 262144 RGBA bytes, so a corrupted tile is detected on load
  (`corrupt-raster`) and a missing one is `missing-resource`.
- `journal` records canonical strokes (brush, samples, color, blend, seed, engine, and
  the resulting `tile_map_sha256`). It is capped at 256 entries; older entries are
  dropped and counted in `checkpoint.journal_dropped`. `raster checkpoint --compact`
  empties the journal explicitly. Tiles, not the journal, are the source of truth.
- Raster nodes require document version 6 and cannot be downgraded to v5.

## Brush engine (engine 1)

Deterministic across platforms: only IEEE `+ - * / sqrt`, `floor`, `round`, integer
math, a fixed 14-term Taylor sin/cos, and a splitmix64 seeded stream (scatter) are used.
Samples are rounded to 1/1000 px. Dabs are placed at `max(spacing*size, 0.25)` px along
the polyline. Per-stroke coverage accumulates in 16-bit with `flow`; `opacity` caps the
stroke; the stroke then composites onto tiles in straight alpha with integer math.

Kinds: `hard-round`, `soft-round` (smoothstep falloff from `hardness`), `pixel`
(unantialiased square, 1 px step), `calligraphic` (rotated ellipse via `angle`,
`roundness`). Properties: `size` 1-2048, `hardness`, `spacing` 0-2, `opacity`, `flow`,
`angle`, `roundness`, `scatter` 0-5, `pressure_size`, `pressure_flow`, `min_size`.
Blend: `normal` or `erase`. Unknown properties are rejected.

## Limits (checked before pixel work)

8192 samples, 100000 dabs, bounded per-stroke work, 4096 tiles, 256 journal entries.

## CLI

```sh
pentool raster doc.pen add paint --width 1200 --height 800
pentool raster doc.pen stroke paint --samples '[[10,60,0.3],[150,30,1]]' \
  --brush '{"kind":"soft-round","size":16,"pressure_size":true}' --color '#D2A184' --seed 1
pentool raster doc.pen --dry-run stroke paint --samples @stroke.json
pentool raster doc.pen info paint
pentool raster doc.pen checkpoint paint --compact
pentool raster doc.pen clear paint
pentool tree doc.pen --kind raster
```

Fixtures: `docs/fixtures/raster-v6.pen` (valid, replays to a recorded hash) and
`docs/fixtures/raster-v6-missing-tile.pen` (invalid). Schema: `raster-paint-v1.schema.json`.

## Not yet implemented in this milestone

Input normalization, clone/heal/fill/selections, merge-down and layer ops, presets,
editor canvas painting, fuzz/performance suites (roadmap items 4-12, 14).
