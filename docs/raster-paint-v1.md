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

### Clone stamp

`raster DOC clone-source TARGET [--layer SOURCE] --x X --y Y` stores a source point
for a target layer in the document-level `raster_clone` map (`layer`, `x`, `y` and
`anchor`). `raster DOC clone-stroke TARGET --samples ... [--brush ...] [--aligned]
[--angle DEG] [--scale S]` then paints with the `clone` blend. It takes dab, size,
hardness, spacing, opacity, flow, tip and dynamics options from `--brush`;
`strength`, `tolerance`, `range` and `mode` do not apply. `--blend clone` on a plain
`stroke` is refused because it has no source.

A destination pixel `p` reads the source at `S + R(-angle) * (p - A) / scale`, with
`S` the source point and `A` the anchor: the stroke's first sample, or, with
`--aligned`, the first sample of the session, kept in `raster_clone` so later
strokes keep one source-to-destination offset. `clone-source` resets the anchor.
Content turns clockwise by `angle` degrees (-360 to 360), `scale` is 0.1 to 10.
Sampling is bilinear on premultiplied color with 8-bit fixed-point weights, with no
minification prefilter. Source pixels outside the source layer are transparent
(clone never clamps to an edge). The sampled color is composited over the
destination with the stroke coverage.

Sources are the target layer (`--layer` omitted, `current` or the target's own id),
read as it was before the stroke, or another raster layer. For another layer the
journal entry records `clone.source` with the layer id, its width and height, and
the digests of exactly the source tiles the stroke can read (at most 256). Replay,
verify and repair load only those digests, and compaction releases them with the
entry, so later edits to the source layer never reinterpret an old clone. A stroke
that would read more than 256 source tiles fails with a request to split it.
Compositing other scene content as a source (the "below" source) arrives with
stamp-visible in item 10.

### Healing

`heal-stroke` takes the same source and options as `clone-stroke` (including
`--aligned`, `--angle` and `--scale`) but paints with the `heal` blend (heal
algorithm 1). The source supplies texture; the surroundings supply tone and color:

- brush `texture` (0-1, default 1) scales the source's detail, which is its
  deviation from its own 5x5 alpha-weighted box mean;
- brush `tone` (0-1, default 1) scales a correction computed from the difference
  between the destination and the textured source on the one-pixel ring around the
  stroked region, filled across the region by harmonic interpolation (fixed row-major
  Gauss-Seidel sweeps in 1/16 fixed point).

`texture` and `tone` are errors for any other blend. The stroked region is limited to
1024 pixels per side and a fixed solver budget (`limit-exceeded`); heal larger areas
in several strokes. All math is integer, and the journal entry carries
`heal: {"algorithm": 1}`; a journal from a newer algorithm is refused on replay.

`heal-spot ID --x X --y Y [--radius R] [--texture T] [--tone T]` heals one round spot
and chooses the source itself: it scores 64 fixed candidate offsets (4 distances times
16 directions) by how well the pixels in a ring around the spot match the pixels at the
same ring shifted by the offset, preferring smooth source interiors, and takes the
lowest score (first on ties). The result reports `heal.source_offset` and `heal.score`;
the stroke is journaled as an ordinary heal stroke with an explicit source. It fails if
the surroundings are not fully painted. Patch healing (selection based) is not
implemented yet; selections now limit heal like every other paint operation.

### Flood fill and selections

`fill ID --x X --y Y --color #RRGGBB [--opacity O] [region options]` fills a flood
region; `select-wand ID --x X --y Y [--mode M] [region options]` selects it instead.
Region options (flood algorithm 1):

- `--tolerance T` (0-255, default 32): a pixel joins when its largest straight-RGBA
  channel difference from the seed is at most T. Fully transparent pixels all count as
  one color; `--transparent-barrier` keeps them out of an opaque seed's region.
- contiguous by default over 4 neighbors; `--diagonal` uses 8; `--global` takes every
  similar pixel in the layer.
- `--gap N` (0-8, contiguous only) erodes the candidates by an N-pixel square first,
  so breaks up to 2N pixels wide stop the fill, then grows the result back inside the
  original candidates. A seed inside such a gap is an error.
- edges are anti-aliased with a 3x3 box average unless `--no-antialias`.
- the sample scope is the layer itself; sampling the visible composite arrives with
  stamp-visible (item 10).

Layers over 32 megapixels are refused before any plane is allocated. Nothing is
written until the whole region is known, so a failed fill leaves the document
byte-for-byte unchanged; there is no partial result to cancel. `fill` rolls the layer's
checkpoint like the other non-stroke operations.

A selection is an 8-bit coverage plane stored in `raster_tiles` (`[0,0,0,coverage]`
tiles) and referenced from `raster_selections.active.<layer id>` as `{width, height,
tiles}`. No entry means no selection (everything is editable); an entry without tiles
selects nothing. `--mode` combines a new region with the current selection: `replace`
(default), `add` (union), `subtract` and `intersect`. `select-info ID` summarizes it
and `select-clear ID` removes it.

While a selection exists for a layer, strokes, clone, heal and fill change only what it
covers, in proportion to coverage (premultiplied mix of the unselected and stroked
result). Each stroke's journal entry pins the selection tiles in `selection`
(at most 256 tiles), so replay never reads the live selection. A selection whose size no
longer matches its layer blocks painting with an error that names `select-clear`.

### Selection shapes, modifiers and selected pixels

Selection algorithm 1. Every command works on the layer's selection plane and takes
`--mode replace|add|subtract|intersect` where it creates a selection.

- `select-marquee ID --rect X Y W H [--shape rect|ellipse] [--feather N]` and
  `select-lasso ID --points '[[x,y],...]' [--feather N]` (3-4096 points, even-odd
  rule). Edges are rasterized with 4x4 sub-pixel sampling, so they carry partial
  coverage; shapes partly outside the layer are clipped, and shapes wholly outside
  are an error.
- `select-quickmask ID --samples ... [--brush ...] [--erase]` paints coverage with
  the brush engine: the same dabs, hardness, flow and dynamics as a color stroke, but
  into the selection plane. Textured tips are not supported.
- `select-modify ID --op OP [--amount N] [--tolerance T]` changes the existing
  selection (an error when nothing is selected):
  `feather` (three box blurs of radius ceil(N/2), N 1-256, edge clamped), `smooth`
  (box blur of radius N, then threshold at 50%), `expand`/`contract` (square
  structuring element, N 1-64; a contraction also eats in from the layer border),
  `border` (a band N pixels wide centered on the edge), `invert`, `grow` (add
  4-connected neighbors whose color is within T of the selected pixel they were
  reached from, for at most N pixels of distance) and `similar` (add every layer pixel
  within T of any selected color; at most 64 distinct selected colors).
- `select-save ID NAME`, `select-load ID NAME [--mode M]` and `select-delete ID NAME`
  keep up to 32 named selections per layer under
  `raster_selections.saved.<layer id>.<name>`; `select-info` lists the names.

Working with the selected pixels (the layer must be unlocked wherever pixels change):

- `lift ID --new-id NEW [--cut]` copies (or cuts) the selection into a new raster layer
  placed directly above, sized to the selection and positioned over it. Pixels keep
  their color and carry `alpha * coverage`, so transparency and soft edges survive.
  Cutting removes the same share from the source. A floating selection is a cut
  lift: move the new layer with the ordinary node commands, then `merge-down`.
- `move-pixels ID --dx DX --dy DY [--copy]` moves the selected pixels and the selection
  by whole pixels; `--copy` leaves the originals.
- `transform-pixels ID [--scale S] [--scale-y S] [--rotate DEG] [--dx X --dy Y]
  [--nearest] [--copy]` scales (0.05-20) and rotates (-360 to 360) the selected pixels
  and the selection about the center of the selection's bounds, then shifts them. The
  default resampling is bilinear on straight alpha; `--nearest` keeps hard pixels.
- `paste ID --source OTHER [--x X --y Y] [--opacity O]` composites another raster
  layer's pixels over this one at a layer-local position; an active selection limits it.

`move-pixels`, `transform-pixels`, `paste` and a cutting `lift` roll the layer's
checkpoint (reasons `move`, `copy-move`, `transform`, `paste`, `cut`) and empty its
journal. Compositing is straight-alpha "over", so alpha is never discarded. Selection
planes over 32 megapixels are refused before allocation.

### Orientation and page-wide composition

Composition algorithm 1. All of these run on a clone, validate, and replace the
document in one transaction; the global `--dry-run` previews them.

- `rotate ID --degrees 90|180|270` (clockwise; `-90` is `270`) and
  `flip ID horizontal|vertical` rewrite the layer's tiles losslessly. A quarter turn
  swaps width and height; the layer keeps its top-left position. The active and saved
  selections of the layer are reoriented with the pixels. Checkpoint reasons are
  `rotate-90`, `rotate-180`, `rotate-270`, `flip-horizontal`, `flip-vertical`.
- `merge-visible ID` merges every visible raster sibling of `ID` into the bottom-most
  one using `merge-down` semantics (opacity, blend mode and space of each upper layer
  are applied). Hidden siblings in between move above the merged layer. A visible
  non-raster node between the rasters is refused rather than reordered.
- `stamp-visible --new-id NEW` composites everything visible on the page, without the
  page background, into a new raster layer trimmed to its painted bounds and placed on
  top of the top layer. Nothing is hidden or deleted. An empty result is an error.
- `flatten --new-id NEW` does the same, then removes every visible node and puts the
  composite in the first layer that held one. Hidden nodes and hidden layers are kept.
  Visible locked nodes or layers block it.

### Brush presets and brush imports

A preset is a named brush stored as readable data in the document's `brush_presets`:
`{"engine": 1, "brush": {...}, "description": "..."}`. The `brush` object has exactly the
properties `--brush` accepts, so a preset cannot hold a script or any property the
engine does not know. A preset needs raster engine <= the one the build implements;
newer presets are refused with a corrective message. Names match
`^[A-Za-z0-9._-]{1,64}$`; a document holds at most 256.

- `preset-add NAME --brush JSON [--description TEXT] [--replace]`, `preset-remove NAME`,
  `presets` (compact list) and `preset-show NAME`.
- `--preset NAME` on `stroke`, `clone-stroke`, `heal-stroke` and `select-quickmask`
  (or `"preset"` inside `--brush`) starts from the preset; any property in `--brush`
  overrides it. The journal records the fully resolved brush plus `preset`, so editing or
  deleting a preset never reinterprets an old stroke, and replay does not read presets.
- A textured preset names a `brush_tips` tip; its pixels are content-addressed in
  `raster_tiles`, like any other tile, and travel with the document.
- `preset-export NAME --out FILE` writes a portable JSON file
  (`pentool_brush_preset: 1`, the brush, and the tip's pixels when it has one).
  `preset-import FILE [--name N] [--replace]` verifies the tip pixels against their
  digest and rejects any property it does not know.

Importing third-party brushes is opt-in, one explicit command per format, and converts
only what has an exact meaning here:

- `preset-import-gbr FILE NAME` reads a GIMP `.gbr` (version 1 or 2, grayscale or RGBA,
  up to 4096 px a side). The pixels become a tip named `NAME` (grayscale paints where
  dark, as in GIMP; RGBA by its alpha; color is dropped because the color is chosen per
  stroke), spacing comes from the header, and the preset is `textured`.
- `preset-import-mypaint FILE NAME` reads a MyPaint `.myb` (`"version": 3`) and converts
  `radius_logarithmic` (to a diameter `2 * e^v`, rounded to 0.01), `hardness`, `opaque`
  (to `flow`), `dabs_per_basic_radius` (to spacing `1/(2d)`) and pressure on radius and
  opacity. Every other setting that is switched on, with a non-zero base value or any
  input curve, is listed under `unsupported` and not approximated; color-choosing
  settings are listed under `ignored_color_settings`.

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

### Batch

`pentool raster doc.pen batch ops.json` applies a JSON array of
`{"action", "id", "args"?, "page"?}` operations in order as one recoverable transaction.
The actions are those of `POST /api/raster` (`stroke`, `quickmask`, `clone`, `heal`, `fill`,
`clear`, `set-clone-source`, `select-marquee`, `select-lasso`, `select-wand`, `select-info`,
`select-clear`, `info`) and `args` mirror the CLI flags, including `preset`. Both entry
points share one dispatcher, so a batch produces the same tile hashes as the same
commands run one by one.

Limits are checked before any pixel work: 1-256 operations, at most 2,000,000 stroke
samples in total, a 64 MiB file, and only the fields `action`, `id`, `args` and `page`.
The summary lists, per operation, the engine's compact result (changed `bounds`,
`tiles_changed`, input summary) and the layer's `tile_map_sha256`; it never contains
pixels. If any operation fails, the error names its index and action and the document is
left byte-for-byte unchanged. `--dry-run` runs the same planner and writes nothing.

## Editor canvas

`pentool serve` embeds a **Raster canvas** panel (`web/raster-panel.js`) for v6 documents.
It never edits pixels itself: on pointer release it sends one request to the stateless
`POST /api/raster` endpoint, `{document, page?, id, action, args}`, and gets back
`{document, result}` or a `{error}` problem. The document is validated before it is
returned and a rejected request changes nothing. Actions: `info`, `stroke` (erase with
`"blend":"erase"`), `quickmask`, `clone`, `heal`, `set-clone-source`, `select-marquee`,
`select-lasso`, `select-wand`, `select-info` and `select-clear`; `args` mirror the CLI
flags (`samples`, `brush`, `preset`, `color`, `blend`, `seed`, `x`/`y`/`width`/`height`,
`points`, `mode`, `feather`, `tolerance`, ...). The browser applies the returned document
as a local change, so Undo and Save work as for any other panel.

Overlays are drawn client-side and are disposable: the brush-size cursor outline, a live
stroke preview, the stabilizer line (smoothing 0-0.95 is the engine's EMA weight; the line
runs from the smoothed dab to the pen), the clone source marker, the quick-mask tint and
marquee/lasso edges. The canvas supports zoom and 15-degree rotation steps; pointer
coordinates are mapped back through both. Pressure comes from pointer events. The preview
image is a bounded proxy rendered by Rust (`/api/render/png`).

Not implemented in this milestone: rulers, guides, snapping, a navigator, tilt/twist from
pointer events, and marching-ants animation of soft selection edges.

## Limits (checked before pixel work)

65536 input events, 8192 samples, 16 MiB per `@file` argument, 100000 dabs, bounded per-stroke work, 4096 tiles, 256 journal entries.

## CLI

```sh
pentool raster doc.pen add paint --width 1200 --height 800
pentool raster doc.pen stroke paint --samples '[[10,60,0.3],[150,30,1]]' \
  --brush '{"kind":"soft-round","size":16,"pressure_size":true}' --color '#D2A184' --seed 1
pentool raster doc.pen --dry-run stroke paint --samples @stroke.json
pentool raster doc.pen stroke paint --samples @stroke.json --blend blur \
  --brush '{"size":48,"strength":0.8}'
pentool raster doc.pen stroke paint --samples @stroke.json --blend color-replace \
  --brush '{"size":40,"tolerance":24}' --color '#3A8F4B'
pentool raster doc.pen stroke paint --samples '[{"x":10,"y":60,"pressure":0.3,"azimuth":40,"t":0},{"x":150,"y":30,"pressure":1,"azimuth":80,"t":48}]' \
  --brush '{"kind":"calligraphic","dynamics":{"angle":{"input":"azimuth"},"size":{"input":"velocity","curve":[[0,0.4],[3,1]]}}}'
pentool raster doc.pen clone-source paint --layer source --x 820 --y 640
pentool raster doc.pen clone-stroke paint --samples @clone-stroke.json --aligned \
  --brush '{"size":24,"hardness":0.6}'
pentool raster doc.pen heal-stroke paint --samples @heal-stroke.json \
  --brush '{"size":30,"texture":0.8}'
pentool raster doc.pen heal-spot paint --x 410 --y 233 --radius 10
pentool raster doc.pen fill paint --x 410 --y 233 --color '#C84B31' --tolerance 24 --gap 2
pentool raster doc.pen select-wand paint --x 40 --y 40 --mode add --global
pentool raster doc.pen select-marquee paint --rect 40 40 200 120 --shape ellipse --feather 6
pentool raster doc.pen select-lasso paint --points '[[10,10],[300,40],[120,260]]' --mode add
pentool raster doc.pen select-quickmask paint --samples @mask.json --brush '{"size":30}'
pentool raster doc.pen select-modify paint --op expand --amount 4
pentool raster doc.pen select-save paint subject
pentool raster doc.pen select-load paint subject --mode subtract
pentool raster doc.pen lift paint --new-id subject-copy --cut
pentool raster doc.pen move-pixels paint --dx 40 --dy -10 --copy
pentool raster doc.pen transform-pixels paint --scale 1.5 --rotate 12
pentool raster doc.pen paste paint --source subject-copy --x 200 --y 100 --opacity 0.8
pentool raster doc.pen rotate paint --degrees 90
pentool raster doc.pen flip paint horizontal
pentool raster doc.pen merge-visible paint
pentool raster doc.pen stamp-visible --new-id stamp
pentool raster doc.pen flatten --new-id flat
pentool raster doc.pen preset-add soft-retouch --brush '{"kind":"soft-round","size":36,"flow":0.12}'
pentool raster doc.pen stroke paint --preset soft-retouch --samples @stroke.json --color '#D2A184'
pentool raster doc.pen preset-export soft-retouch --out soft-retouch.preset.json
pentool raster doc.pen preset-import-gbr chalk.gbr chalk
pentool raster doc.pen preset-import-mypaint pencil.myb pencil
pentool raster doc.pen select-info paint
pentool raster doc.pen select-clear paint
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

Patch healing, the stamp-visible clone source and composite flood scope, the editor's rulers/guides/snapping/navigator, fuzz/performance suites (roadmap items 6-7, 12, 14).
