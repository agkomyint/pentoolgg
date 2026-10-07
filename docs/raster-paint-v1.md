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
  `brush_tips`, `raster_selections` and `brush_presets`.

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
`roundness`), `textured` (a stamp tip, below). Properties: `size` 1-2048, `hardness`,
`spacing` 0-2, `opacity`, `flow`, `angle`, `roundness`, `scatter` 0-5,
`pressure_size`, `pressure_flow`, `min_size`, `smoothing` 0-0.95, `buildup`, `tip`,
`dynamics`, and the local-tool parameters `strength`, `tolerance`, `range` and `mode`
(below). Blend: `normal`, `erase` or one of the local tools below. Unknown properties
are rejected.

- `smoothing`: an exponential moving average over position and pressure,
  `p[i] = p[i-1] + (1 - smoothing) * (raw[i] - p[i-1])`, starting at the first
  sample. If the average lags, the raw last sample is appended so the stroke still
  ends where the pen lifted. 0 (the default) leaves samples untouched.
- `buildup`: `true` (the default) accumulates overlapping dabs,
  `c += dab * (1 - c)` in 16-bit. `false` keeps the per-pixel maximum, so one stroke
  never exceeds a single dab's `flow`.
- `textured`: `tip` names a `brush_tips` entry or gives its `sha256:` digest. The
  tip is a 256x256 coverage plane stored as an ordinary `raster_tiles` tile, and
  coverage is its alpha channel. The tip square spans the dab diameter and is
  rotated by `angle`. Its height is scaled by `roundness`, and it is sampled
  bilinearly with zero outside the square. Tip names are resolved before a stroke
  is journaled, so journals always record the digest and replay never depends on
  a name.

- `dynamics`: maps `size`, `flow`, `roundness` or `angle` to
  `{input, curve, fallback}`. `input` is `pressure`, `tilt` (0-90°), `azimuth`
  (0-360°), `twist` (0-360°) or `velocity` (px/ms, 0-100). `curve` holds 2-16
  `[input, output]` points with strictly increasing inputs inside the input's
  range. It is piecewise linear and clamps beyond its end points. `size`, `flow`
  and `roundness` multiply the brush value, with outputs 0-1 and roundness floored
  at 0.05. `angle` adds degrees, with outputs from -360 to 360. If `curve` is
  omitted, multipliers ramp from 0 to 1 across the input range, and `angle` follows
  `tilt`, `azimuth` or `twist` one-to-one. `fallback` is the input value used when
  a stroke's samples do not report that channel, for example a mouse; it defaults
  to 1 for pressure and 0 otherwise. `pressure_size` cannot be combined with
  `dynamics.size`, and `pressure_flow` cannot be combined with `dynamics.flow`.
  Channels are interpolated per dab, and azimuth and twist take the shorter way
  around the circle. `smoothing` affects only position and pressure.

Properties added after the first engine-1 release (`smoothing`, `buildup`, `tip`, `dynamics`) are
written to the journal only when they differ from their defaults, so existing
journals, stroke IDs and replay hashes are unchanged.

### Erasing and local blending

`--blend` selects what a stroke does to the pixels its dabs cover. Dab shape, size,
hardness, spacing, flow, opacity, smoothing, tips and dynamics work exactly as for
painting. The per-pixel stroke coverage (0-65535, flow and opacity included) is
the `amount` below.

- `normal`: paints `--color`. `erase`: pixel eraser, lowers alpha by `amount`
  (`clear` removes a layer's pixels outright, and a fully erased tile is dropped).
- `background-erase`: samples the color under the first dab, which must be inside
  the layer and not transparent. Pixels whose largest channel difference from the
  sample is within `tolerance` (0-255, default 32) lose alpha by `amount * w`, where
  `w = (tolerance + 1 - distance) / (tolerance + 1)`. Other pixels are untouched,
  which protects the subject.
- `color-replace`: the same sample, tolerance and weight. Matching pixels shift by
  `(--color - sample) * amount * w`, per channel and clamped, so shading survives.
- `blur`: lerps each pixel toward the mean of a `(2r+1)` square box, with
  `r = clamp(round(size / 16), 1, 4)`, in premultiplied color including alpha, by
  `amount * strength`.
- `sharpen`: unsharp mask on premultiplied color with that box and gain 2,
  `p + 2 * (p - mean) * amount * strength`. Alpha is not changed.
- `dodge` / `burn`: lighten `c += (255 - c) * e` or darken `c -= c * e`, where
  `e = amount * strength * weight(luma)`. `range` picks the weight: `shadows`
  `(255-l)^2/255`, `highlights` `l^2/255`, `midtones` `255 - |2l - 255|`
  (default). `luma = (54R + 183G + 19B + 128) >> 8`. Alpha is not changed.
- `sponge`: moves each channel away from (`mode: saturate`) or toward
  (`desaturate`, the default) its luma by `amount * strength`.
- `smudge`: the only tool that reads its own output. A carried premultiplied color
  starts as the pixel under the first dab. Each dab moves covered pixels toward the
  carry by `amount * strength`, then moves the carry toward the pixel under the dab
  center (read before the dab) by `1 - strength`. At strength 1 the carry never refreshes.

`strength` (0-1, default 0.5) applies to smudge, blur, sharpen, dodge, burn and sponge.
`tolerance` applies to background-erase and color-replace, `range` to dodge and
burn, and `mode` to sponge. A parameter given to a blend that does not use it is
`[invalid-brush]`, never silently ignored. Parameters are journaled only when given.

Every tool except smudge reads from the layer as it was before the stroke, so the
result does not depend on dab order or tile boundaries. Reads beyond the layer edge
clamp to the nearest edge pixel, and writes are clipped to the layer. All math is
integer, with rounding half away from zero. Kernel tools multiply the stroke-work
estimate by `(2r+1)^2`, and the limit is checked before pixel work.

Selections and layer masks: a raster layer's own mask is a non-destructive display
mask and does not restrict painting. A pixel selection that multiplies `amount`
arrives with roadmap item 9; until then the whole layer is selected.

### Input normalization

`stroke --samples` takes device events. Each event is `[x, y]`, `[x, y, pressure]` or
`{x, y, pressure?, tilt?, azimuth?, twist?, velocity?, t?}`, where `t` is in
milliseconds. A device reports only the channels it has. Every optional channel must
appear on all events of a stroke or on none, and unknown channels are rejected.
Events are normalized once, before anything is journaled:

1. Validation: pressure 0-1 (default 1), tilt 0-90, azimuth and twist 0-360 (360
   wraps to 0), velocity 0-100, and `t` non-decreasing. `t` and `velocity` cannot
   both be given.
2. Velocity: with `t`, each event gets `distance / dt` in px/ms, clamped to 100.
   A zero `dt` repeats the previous value, and the first event copies the second.
3. Folding: an event closer than 0.5 px to the last kept sample is dropped. The
   final event is always kept.
4. Values are rounded to 1/1000. At most 65536 events are accepted, and they must
   fold to 8192 samples or fewer.

The journal records only the canonical samples. Samples without extended channels
are `[x, y, pressure]` arrays, as before. Others are objects carrying `velocity`
but never `t`, so replay does not depend on event timing. The stroke result reports
`input: {events, samples, folded}`. Palm rejection and gestures are editor
concerns: they decide which events form a stroke, but they never alter this math.

### Brush tips

`tip-add NAME --image FILE [--source darkness|alpha]` reads a PNG, JPEG or WebP
image of up to 16 MiB through the bounded image decoder. With `darkness` (the
default), coverage is `(255 - luma) * alpha`, where
`luma = (54R + 183G + 19B + 128) >> 8`, so black paints. With `alpha`, coverage is
the alpha channel. The image is fitted so its longer side is 256, using the
deterministic premultiplied bilinear resampler, and is centered. Color channels are
stored as zero, so identical tips share one digest.

Document-level `brush_tips` maps a name (1-64 of `A-Z a-z 0-9 . _ -`) to a digest;
at most 256 names. Named tips are always retained. A tip that is only in a journal
is kept while that journal entry exists. `checkpoint --compact` releases it, and
the result reports `tiles_released`. `tip-remove NAME` drops the name, and `tips`
lists the names. `verify` decodes every named tip and reports `tips.damaged` as
`missing` or `corrupt`. A damaged tip cannot be rebuilt: re-add it from its image.

## Layer operations

None of these is replayable stroke math, so each one rolls the layer checkpoint
(`checkpoint.reason` names the operation) and empties the journal. Each runs in one
transaction with dry run, `--if-revision` and undo, and a failure leaves the file
byte-for-byte unchanged.

- `resize ID --width W --height H [--resample bilinear|nearest]`: scales pixels;
  position is kept. Bilinear runs in premultiplied alpha with pixel-center mapping.
  Working surfaces are limited to 32 Mi pixels (`MAX_TEMP_BYTES / 8`).
- `crop ID --x X --y Y --width W --height H`: a layer-local rectangle without
  resampling. It may extend past the old bounds, which makes it the layer
  canvas-resize operation as well; pixels keep their page position and
  `pixels_removed` reports what fell outside.
- `trim ID`: crop to the painted bounds. An empty layer is refused.
- `duplicate ID --new-id NEW`: inserted directly above; tiles are shared, not copied.
- `merge-down ID`: composites the layer into the raster sibling directly beneath it
  with its opacity and blend mode. The lower layer grows to the union, so no pixels
  are discarded. Merging is refused, without mutation, when the result could not
  match what was shown. This covers scale, rotation, skew, warp or fractional
  offsets; effects, masks, clipping or content opacity on either layer; and
  opacity or blend on the lower layer. In those cases, use `rasterize` first.
  The merged layer is stored as 8-bit straight alpha before it meets the page
  backdrop, so soft edges may differ from the unmerged render by one code value.
- `rasterize ID --new-id NEW [--replace]`: renders any node exactly as the compositor
  does, which bakes its effects, masks and children. The result is trimmed to its
  alpha bounds and becomes a raster layer directly above the source. The source is
  hidden (`visible:false`), not deleted, unless `--replace` is given. Nodes inside
  a transformed group are refused; rasterize the group instead.

Raster nodes take part in groups, masks, clipping, effects, opacity, blend modes,
components and packages through the shared scene paths. Tile retention walks the
whole document, including component snapshots and instance fallbacks. `diff`
reports `/raster_tiles/<digest>` changes as `{raster_tile, encoding, data_bytes}`
summaries, never base64 data.

## Limits (checked before pixel work)

65536 input events, 8192 samples, 16 MiB per `@file` argument, 100000 dabs, bounded per-stroke work, 4096 tiles, 256 journal entries.

## CLI

```sh
pentool raster doc.pen add paint --width 1200 --height 800
pentool raster doc.pen stroke paint --samples '[[10,60,0.3],[150,30,1]]' \
  --brush '{"kind":"soft-round","size":16,"pressure_size":true}' --color '#D2A184' --seed 1
pentool raster doc.pen --dry-run stroke paint --samples @stroke.json
pentool raster doc.pen stroke paint --samples @stroke.json --blend blur \n  --brush '{"size":48,"strength":0.8}'
pentool raster doc.pen stroke paint --samples @stroke.json --blend color-replace \n  --brush '{"size":40,"tolerance":24}' --color '#3A8F4B'
pentool raster doc.pen stroke paint --samples '[{"x":10,"y":60,"pressure":0.3,"azimuth":40,"t":0},{"x":150,"y":30,"pressure":1,"azimuth":80,"t":48}]' \
  --brush '{"kind":"calligraphic","dynamics":{"angle":{"input":"azimuth"},"size":{"input":"velocity","curve":[[0,0.4],[3,1]]}}}'
pentool raster doc.pen tip-add grain --image grain.png --source darkness
pentool raster doc.pen stroke paint --samples @stroke.json \
  --brush '{"kind":"textured","tip":"grain","size":48,"smoothing":0.6,"buildup":false}'
pentool raster doc.pen tips
pentool raster doc.pen tip-remove grain
pentool raster doc.pen info paint
pentool raster doc.pen checkpoint paint --compact
pentool raster doc.pen verify --replay
pentool raster doc.pen repair paint --strategy replay
pentool raster doc.pen clear paint
pentool raster doc.pen resize paint --width 600 --height 400 --resample bilinear
pentool raster doc.pen crop paint --x -20 --y 0 --width 1240 --height 800
pentool raster doc.pen trim paint
pentool raster doc.pen duplicate paint --new-id paint-copy
pentool raster doc.pen merge-down paint-copy
pentool raster doc.pen rasterize card --new-id card-pixels
pentool tree doc.pen --kind raster
```

Fixtures: `docs/fixtures/raster-v6.pen` (valid, replays to a recorded hash),
`docs/fixtures/raster-v6-missing-tile.pen` (invalid: `missing-resource`) and
`docs/fixtures/raster-v6-corrupt-tile.pen` (a tile whose bytes do not match its digest;
`verify --replay` reports it `rebuildable` and `repair` restores the recorded hash).
Schema: `raster-paint-v1.schema.json`.

## Not yet implemented in this milestone

Clone/heal/fill/selections, pixel-selection limits for the local tools, rotate/flip, merge visible, stamp
visible, flatten, presets, editor canvas painting, fuzz/performance suites (roadmap
items 6-12, 14).
