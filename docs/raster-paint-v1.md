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
  the resulting `tile_map_sha256`). Tiles, not the journal, are the source of truth.
- Raster nodes require document version 6 and cannot be downgraded to v5.

## Checkpoints and replay

A `checkpoint` pins a full tile map (`tiles`) plus `journal_offset`. Replaying
`journal[journal_offset..]` from those tiles must reproduce the live tile map, and each
entry's recorded `tile_map_sha256` is checked along the way. A layer without a
checkpoint replays from transparency.

- `raster checkpoint ID` pins the current tiles and sets `journal_offset` to the journal
  length; `--compact` also empties the journal (`journal_offset` 0).
- When a stroke fills the journal to 256 entries, the layer rolls automatically: the
  checkpoint becomes the new tiles, the journal empties, and `journal_dropped` counts
  every entry ever compacted. Replay is therefore bounded by 256 entries per layer.
  Stroke IDs (`s<index>-<hash>`) keep counting across rolls.
- Garbage collection keeps every `sha256:` digest a raster node pins (live tiles,
  checkpoint tiles, digests recorded in journal entries) plus those in document-level
  `raster_selections` and `brush_presets`.

## Corruption recovery

`raster verify [ID] [--replay]` is read-only. It decodes every referenced tile, checks
its digest, reports `missing` and `corrupt` tile keys per layer, checkpoint health, and
unreferenced store entries, and exits nonzero when anything is damaged. With
`--replay` it also replays each journal: `match`, `mismatch`, `rebuildable` (live tiles
are damaged but replay reproduces them) or `failed`.

`raster repair ID --strategy replay` rebuilds the exact pixels from the checkpoint and
journal and rewrites damaged store entries; it fails without writing when replay is
impossible. `--strategy transparent` keeps intact tiles, drops damaged ones to
transparency, lists them in `checkpoint.repaired_lost_tiles`, and rolls a checkpoint
because the old journal no longer describes the pixels. Both run as one transaction
with `--dry-run`, revision guards and undo. Repair never guesses pixels.

## Compatibility

The node `engine` records the raster engine that last wrote it (absent means 1).
Every reader renders, exports, inspects and verifies a layer from any engine because
its tiles are materialized. A writer refuses to mutate, repair or replay a layer,
checkpoint or journal entry whose engine is newer than its own
(`unsupported-capability`). A different `tile_size` or `pixel_format` is rejected on
open, because readers cannot interpret those pixels.

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
pentool raster doc.pen verify --replay
pentool raster doc.pen repair paint --strategy replay
pentool raster doc.pen clear paint
pentool tree doc.pen --kind raster
```

Fixtures: `docs/fixtures/raster-v6.pen` (valid, replays to a recorded hash),
`docs/fixtures/raster-v6-missing-tile.pen` (invalid: `missing-resource`) and
`docs/fixtures/raster-v6-corrupt-tile.pen` (a tile whose bytes do not match its digest;
`verify --replay` reports it `rebuildable` and `repair` restores the recorded hash).
Schema: `raster-paint-v1.schema.json`.

## Not yet implemented in this milestone

Input normalization, clone/heal/fill/selections, merge-down and layer ops, presets,
editor canvas painting, fuzz/performance suites (roadmap items 4-12, 14).
