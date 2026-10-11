# Agent-painting protocol v1

Status: frozen data contract; mutation commands are not implemented by this
document. This protocol is the prerequisite for v0.13 roadmap items 2-14.

The protocol converts compact artistic intent into the canonical raster samples
defined by [`raster-paint-v1.md`](raster-paint-v1.md). The Rust CPU renderer and
raster engine remain authoritative. A conforming implementation must not invoke a
model, script, plugin, network service, or GPU-only path while resolving, compiling,
previewing, committing, replaying, or comparing these objects.

Normative schemas:

- [`agent-painting-request-v1.schema.json`](agent-painting-request-v1.schema.json)
  describes one high-level path compilation request.
- [`paint-plan-v1.schema.json`](paint-plan-v1.schema.json) describes an ordered,
  data-only multi-pass plan.
- [`agent-painting-result-v1.schema.json`](agent-painting-result-v1.schema.json)
  describes stable success and error envelopes.

The schemas validate representation. This document also imposes semantic rules
which JSON Schema cannot express, including unique identifiers, reference
resolution, exact hashes, aggregate budgets, finite arithmetic, and document
revision checks.

## Versions and compatibility

`schema` is `1`. `engine` is the positive integer agent-painting compiler version;
version 1 compiles to raster engine 1 samples. Readers may inspect unknown engines,
but a writer must return `unsupported-capability` before pixel work when any source,
dependency, field, plan, or journal entry requires a newer engine. Materialized
tiles remain renderable according to the raster storage contract.

Compilation inputs include the normalized request, compiler engine, referenced
document revision, exact dependency bytes, resolved scope masks/fields, palette,
and seed. Identical inputs must produce identical canonical JSON, samples, dabs,
tile hashes, and results on every supported release target.

Unknown fields are rejected in protocol objects. This prevents a misspelled safety
constraint from being silently ignored. Future optional behavior requires a new
schema or engine version. Stored `.pen` extension containers may retain unknown
fields under their existing format rules, but those fields do not affect protocol
execution.

## Canonical JSON and hashes

All `sha256:` values use lowercase hexadecimal SHA-256. Content hashes cover UTF-8
RFC 8785 JSON Canonicalization Scheme bytes for JSON values and exact bytes for
binary dependencies. Numbers must be finite. Negative zero normalizes to zero.
Coordinates, normalized distances, and curve outputs are rounded to the nearest
one-thousandth, ties away from zero, before canonical samples are emitted.

`request_hash` covers the normalized request excluding transport-only `dry_run`,
`if_revision`, and cancellation token. `source_hash` covers the exact rendered or
stored source used by a scope. `constraint_hash`, `field_hash`, and `palette_hash`
cover fully resolved data, not a mutable name. `plan_hash` covers the normalized
plan plus an ordered dependency table of `(kind, id, hash)`. `compiled_hash`
covers the ordered canonical operations emitted by compilation.

Names are resolved once during planning. Commit must use the resolved hashes from
that plan; it must never resolve a name again and accept changed content. A hash
mismatch is `dependency-changed` and causes no mutation.

## Semantic paths and curves

A path uses absolute SVG-compatible `M`, `L`, `Q`, `C`, and `A` segments. It has
exactly one initial `M`, no subpaths, and no `Z`. Arc rotation is degrees; flags
are booleans. Coincident endpoints and zero-radius arcs compile as a line. Paths
must contain at least one drawing segment and at most 4,096 drawing segments.

Compiler 1 converts quadratics and arcs to cubics, then recursively subdivides
curves at `t = 0.5` until maximum control-point distance from the chord is at most
`flatness` (default 0.25 px, range 0.01-4) or depth 16 is reached. Reaching depth
16 without meeting flatness is `limit-exceeded`. Arc conversion follows SVG 2
endpoint-to-center correction and splits sweeps into pieces no larger than 90
degrees. Degenerate zero-length segments contribute no length.

Arc length is the sum of flattened chord lengths. Normalized distance `u` is
accumulated over this polyline. The first endpoint is always emitted; the final
endpoint is emitted exactly once. Interior segment joins are emitted once. A cusp
does not receive implicit smoothing. Raster spacing inserts samples along this
polyline using the existing raster engine rule.

Property curves contain 2-64 points `[u,value]`, sorted with unique `u`, beginning
at 0 and ending at 1. Interpolation is piecewise linear in `u`. Values are clamped
only where the schema explicitly gives a closed range; out-of-range input is an
error. Color curves interpolate straight, unassociated sRGB8 channels and alpha
with integer half-up rounding. Shortest-path angular interpolation breaks an exact
180-degree tie clockwise. Compiler 1 supports size, pressure, flow, opacity,
rotation, and color curves. Their values multiply or replace the base brush value
as stated by the request schema; absent curves leave the brush unchanged.

## Scopes, constraints, palettes, and fields

Every scope identifies one page and one bounded rectangle, with an optional layer,
node, active/saved selection, or composite source. Planning resolves it to pixels
or a coverage plane and records dimensions plus `source_hash`. Empty scopes are a
successful explicit `empty` state, never an invented zero-valued measurement.

Constraints are ordered: `inside`, `outside`, and opaque clipping establish base
coverage; every `avoid` mask subtracts coverage; scalar barriers then stop path or
field traversal; direction modes affect orientation only. Coverage is unsigned
8-bit and combination uses integer multiplication with `(a*b+127)/255`. Evaluation
uses global layer coordinates, so tile boundaries cannot change an answer.

Palette entries are straight sRGB8 RGBA values. A sampled palette records its scope,
algorithm version, source hash, requested role, and ordered colors. Mixing is
linear per channel in sRGB8 for engine 1. Seeded variation uses SplitMix64 from the
raster contract and consumes values in the documented field order. A named palette
must resolve to its exact `palette_hash` before compilation.

A field is a bounded row-major vector grid. Engine 1 vectors are signed 16-bit
fixed-point `(x,y)` values divided by 32767, with `(0,0)` meaning undefined. Its
hash covers method, source hash, bounds, cell size, dimensions, vectors, and method
parameters. Bilinear interpolation renormalizes with integer square-root math; a
zero result is undefined. A committed operation pins `field_hash` and does not
depend on a disposable cache.

## Color sampling and palettes (engine 1 implementation)

`pentool paint-palette FILE <action>` implements the palette rules above.

- Scopes: `page`, `node:ID`, `layer:ID`, `group:ID`, optionally cropped with
  `--rect x,y,w,h` and weighted by a selection `--query`. Pixels come from the
  authoritative renderer; `source_hash` is SHA-256 over the cropped RGBA bytes and
  coverage plane with the dimensions.
- Point sampling returns the weighted mean of a square of radius `0..64`; fully
  transparent or unselected samples report `empty: true`, never an invented color.
- Roles: `dominant` ranks 5-bit-per-channel bins by alpha*coverage weight (ties by
  bin key) and reports each bin's pixel mean; `median` is weighted median cut
  (heaviest box, widest channel, ties by channel order); `shadow` (luma <= 85),
  `midtone` (86..170) and `highlight` (>= 171) use the protocol luma and then rank
  dominant bins; `edge` ranks the pixels whose edge score is at least 96.
- `mix` is linear per channel in sRGB8 with the amount quantized to thousandths and
  rounded half up. `vary` draws one SplitMix64 value per R, G, B in that order for
  each output color and maps it to `[-amount, amount]` with a modulus, clamping
  to 0..255.
- Stored palettes live under `paint_palettes` (at most 256 palettes of 1-256
  colors). `palette_hash` is SHA-256 over `pentool-palette-v1
` and the RGBA
  bytes; a stored hash that does not match its colors is a validation error.

## Region constraints (engine 1 implementation)

`pentool raster FILE stroke-path ID --constraints JSON` (and the `constraints`
request field) confine a semantic stroke to a resolved coverage mask.
`pentool paint-constraints FILE preview ID --spec JSON [--out mask.png]` resolves
the same spec read-only. Spec keys (unknown keys are rejected):

- `inside`: list of `"active"` / `"saved:NAME"`; the mask is their intersection.
- `avoid`: same sources; their coverage is subtracted.
- `opaque_clip`: `true` (or 1..255 minimum alpha) keeps only existing pixels.
- `barriers`: `{kind:color,color,tolerance<=1020}`, `{kind:alpha,min,max}`,
  `{kind:luma,min,max}` or `{kind:edge,threshold 1..2040}` (Sobel on
  alpha-weighted luma); matching pixels are excluded.
- `reach_from`: `[x,y]` seed points; only 4-connected paintable pixels reachable
  from a seed remain, so barriers stop flood-like painting.
- `follow`: `"contour"`, `"gradient"` or `{"angle":degrees}` sets the brush angle
  per path segment from an integer Sobel field (integer CORDIC atan2).

An existing active selection is intersected with the mask and restored (not
altered) afterwards. All arithmetic is integer; the mask is a single plane over
the layer so tile boundaries cannot cause seams or double application. The result
and journal entry carry `mask_hash`, `spec_hash`, `allowed_pixels`, and, for
`follow`, the field digest. A mask with no paintable pixel fails with
`[empty-constraint]` and leaves the document unchanged. `follow` and a rotation
curve are mutually exclusive.

## Stroke fields (engine 1 implementation)

`pentool paint-fields FILE preview ID --field JSON [--params JSON]` builds a field
and reports `field_hash`, `defined_cells`, up to `--max-vectors` vectors
`[x,y,vx,vy]` (divided by 32767), and, with `--params`, the stroke plan
(`strokes`, `samples`, `bounds`). `pentool raster FILE stroke-field ID --field
JSON --params JSON` paints that exact plan; preview and commit share one planner.

Field methods: `contour`, `gradient` (integer Sobel on alpha-weighted luma),
`distance` (3-4 chamfer transform of alpha >= 128; `along` contour|gradient),
`tensor` (smoothed structure tensor, `radius` 1-8, follows the contour),
`vanishing` (`point`), `radial` (`center`, tangents), `angle` (`degrees`) and
`vectors` (`points` `[x,y,degrees]`, nearest anchor wins, lowest index on ties).
`cell` is 1-64 px and `rect` bounds the field; at most 1,048,576 cells.
Distance and tensor fields require at most 16,777,216 layer pixels. Fields are
undirected orientations: interpolation sign-aligns neighbours and a stroke picks
the sign that continues its heading.

Stroke rules (`params`, unknown keys rejected): `spacing` (seed lattice px, 1-256,
default 8), `jitter` 0-1, `length` px, `length_jitter` 0-1, `step` 0.5-16 px,
`curvature` max degrees turned per step, `overlap` 0-1, `taper` 0-0.5 (pressure
ramps over that fraction at each end; enables pressure size), `collision`
`stop|skip|allow`, `min_length`. Seeds consume four SplitMix64 values per lattice
point in row-major order whether used or not, so plans are stable under
unrelated parameter changes of other points. Occupancy uses cells of
`size * (1 - overlap)` px. A plan has at most 512 strokes of 512 samples;
exceeding that is `limit-exceeded` and leaves the document unchanged. Region
`constraints` (without `follow`) bound seeds and walking. Each committed
stroke's journal entry carries `field: {algorithm, field_hash, method}`; the
field cache is disposable and never trusted over the hash.

## Comparison metrics

Comparison inputs are immutable, equally sized, bounded RGBA8 crops produced by
the authoritative renderer. Before comparison, each crop is composited over the
same explicit RGBA8 background using the raster engine's integer source-over math.
No resizing, registration, color-profile guessing, or hidden segmentation occurs.
An empty crop is `empty`; unequal dimensions are `invalid-request`.

Engine 1 reports these measurements. `N` is the crop pixel count, luma is
`(54R + 183G + 19B + 128) >> 8`, and every division is returned as an exact
numerator/denominator plus a decimal rounded to six places, ties away from zero.

- `color_mae`: sum of absolute R, G, and B differences over `3 * 255 * N`.
- `value_mae`: sum of absolute luma differences over `255 * N`.
- `edge_disagreement`: XOR count of edge pixels over `N`. An edge pixel has
  `abs(right-left) + abs(down-up) >= 96`, using clamped luma neighbors.
- `silhouette_iou`: intersection over union of pixels with alpha at least 128.
  A zero union has `value: null` and state `empty`.
- `structure_mae`: for row-major non-overlapping 8x8 blocks (smaller blocks at
  right/bottom edges), compare integer mean luma and edge coverage; the numerator
  is the sum of both absolute differences and the denominator is
  `2 * 255 * block_count`.
- `texture_frequency_mae`: bucket absolute four-neighbor Laplacian luma into
  `[0..7]`, `[8..15]`, `[16..31]`, `[32..63]`, `[64..127]`, and `[128..1020]`.
  It is half the L1 distance between the two six-bin histograms over `N`.

Heat maps, when explicitly requested, encode per-pixel absolute value difference
as a grayscale PNG and are subject to the preview pixel and artifact byte limits.
Metric output is evidence about these formulas only; it is not a beauty, identity,
style-fidelity, or perceptual-equivalence score.

### Region previews and comparison (engine 1 implementation)

`pentool paint-review FILE preview` renders `page`, `node:ID`, `layer:ID` or
`group:ID` at scale 1, optionally crops to `--rect x,y,w,h` or to the bounds of a
`--selection` query (pixels outside the selection are cleared), box-filters it
(premultiplied) to at most `--max-edge` (default 1024) and `--max-pixels`
(default 4,194,304), optionally composites over `--background`, and writes a PNG
only when `--out` is given. Node scopes are page-canvas sized, so rectangles are
page pixels. Output names the source hash (`sha256:` of the RGBA crop) so a
preview is tied to the exact pixels it came from.

`pentool paint-review FILE compare` measures the same crop against exactly one
immutable reference (`--reference-image` PNG/JPEG/WebP, or `--reference-document`
with optional `--reference-scope`), both composited over `--background`
(default `#ffffff`). It reports the six metrics above plus `changed_bounds`
(`[x,y,w,h]` of differing pixels, or `null`). `--heatmap out.png` is the only way
a heat map is produced. Neither action mutates a document.

## Plans and atomic execution

Planning performs all parsing, validation, name resolution, hashing, estimation,
lock checks, revision checks, and budget checks without changing the document.
Passes execute in array order and IDs are unique. Every pass names its target,
brush, blend, opacity, seed, bounded scope, and exactly one source (`path`, `field`,
or explicit canonical operations). References must resolve on the declared page.

Dry run and commit call the same planner. A dry run returns `commit.if_revision`,
`plan_hash`, resolved dependencies, estimates, and the exact normalized command
inputs needed to commit. Commit requires the same document revision and plan hash.
One successful plan is one transaction and one history entry. Partial success is
forbidden: failure, cancellation, stale revision, changed dependency, lock, or
budget exhaustion leaves the original document byte-for-byte unchanged and creates
no history entry, tiles, cache authority, or orphan resources.

## Composition and form guides (engine 1 implementation)

Guides are ordinary document data in the top-level `paint_guides` map, keyed by a
stable id (`[A-Za-z0-9][A-Za-z0-9._-]*`, at most 256 per document). A record holds
`kind`, validated `params`, `label`, `visible`, `proof`, `algorithm` and
`guide_hash` (sha256 over algorithm, kind and params). Kinds: `thirds`, `golden`,
`symmetry`, `horizon`, `perspective`, `vanishing-point`, `gesture`, `proportion`
(`figure-8`, `figure-7.5`, `face`), `value-map` and `custom`. Unknown parameters
fail with `[invalid-request]`. Geometry (lines `[x1,y1,x2,y2]` and anchor points,
rounded to 1/1000) is derived deterministically and never stored.

`pentool paint-guide DOC add|set|remove|list|show|snap|proof` manages them. Adding,
editing, hiding and removing a guide changes guide data only and never a raster
tile (`pixels_changed: false`). Guides do not render or export. `proof --out F.svg
--width W --height H` writes the guides marked `--proof` (or `--ids`, visible ones
unless `--include-hidden`) as an SVG overlay. `snap --id --point x,y --radius R`
returns the nearest anchor point, else the nearest point on a line, within the
radius. A stroke field may reference a guide with
`{"method":"guide","guide":"ID","cell"?,"rect"?}`: vanishing-point and perspective
guides become a `vanishing` field, horizon an `angle 0` field, gesture and custom
guides a `vectors` field from their segments.

## Diagnostics and revision scoring (engine 1 implementation)

Every `paint-plan` pass report carries `diagnostics`: `measured` facts and
`warnings`, each tagged `basis: measured|heuristic`. Optional pass fields `region`
(`[x,y,w,h]`, where the edit should land), `protect` (up to 64 rectangles that must
not change) and `palette` (`#rrggbb` list) enable the matching measurements. Measured:
`changed_pixels`, `changed_tiles`, `changed_bounds`, `coverage` (of the layer),
`value_range` (luma of changed painted pixels), mean colour before/after, up to five
`dominant_colors`, `palette_drift` (mean RGB distance to the nearest palette colour),
`edge_spill_pixels` and `protected_violations`. Warnings: `no-change`,
`protected-region-violation`, `edge-spill` (measured) and `flat-values`,
`large-change` (heuristic suggestions).

`pentool paint-score BEFORE.pen AFTER.pen --layer ID --goal JSON|@file` scores two
revisions against a declared goal (`region`, `palette`, `value_range`, `coverage`,
`reference_layer`, `protect`). Each declared criterion yields a 0-1 score; the
composite is their equal-weight mean and `assessment.trend` (`improved`, `unchanged`,
`worse`, +/-0.02) is explicitly `basis: heuristic`. The command is read-only and
reports `applied: false`; it never accepts, reverts or continues a plan.

## Paint plans (engine 1 implementation)

`pentool paint-plan DOC PLAN.json [--dry-run] [--if-revision REV]
[--expect-plan-hash sha256:...]` runs an ordered, atomic plan. Authoring form:
`{"schema":1,"engine":1,"id","page"?,"description"?,"limits"?,"passes":[{"id",
"target","action","args"?,"description"?}]}`. `action` is one of `stroke`,
`stroke-path`, `stroke-field`, `fill`, `clear`, `clone`, `heal` and `args` is exactly
what the matching `raster` command / `POST /api/raster` accepts (brush, preset,
palette colour, constraints, field, seed, curves), so a pass can do nothing the
single command could not. Unknown fields, duplicate pass ids, other actions, an
unsupported schema or engine and more than 64 passes fail with `[invalid-request]`,
`[unsupported-capability]` or `[limit-exceeded]` before any pixel work.

`limits` may only lower the hard limits (`max_passes` 64, `max_dabs` 4,000,000,
`max_changed_pixels` 67,108,864, `max_tiles` 4,096). Every pass runs on a clone;
its report lists `strokes`, `dabs`, `changed_pixels`, `changed_tiles`,
`changed_bounds` (`[x0,y0,x1,y1)` or `null`), the layer `tile_map_sha256` and the
action result. Exceeding a limit, any failing pass, a stale `--if-revision` or a
mismatched `--expect-plan-hash` (`[plan-changed]`) leaves the document
byte-for-byte unchanged. Success is one transaction and one history entry, so one
`undo` removes the whole plan. The dry-run result equals the commit result;
`plan_hash` covers the canonical plan and `commit.if_revision` is the revision a
commit must pass back.

## Bundles, templates, diff and packages (engine 1 implementation)

`pentool paint-bundle DOCUMENT` exports and imports a data-only bundle:
`{"schema":1,"kind":"pentool-paint-bundle","engine":1,"brushes","palettes",
"guides","plans","bundle_hash"}`. Unknown top-level fields are rejected
(`[invalid-request]`), a mismatched `bundle_hash` is `[conflict]`, and a newer
engine is `[unsupported-capability]`. Every entry passes the validator its own
command uses (`Brush::parse`, palette colors and hash, guide geometry and
`guide_hash`, plan parse and canonical hash). Existing names conflict unless
`--replace`; a failed import writes nothing. Import edits only the document-level
maps `brush_presets`, `paint_palettes`, `paint_guides` and `paint_plans`; scene
nodes, component instances, masks and overrides are never touched, and commits use
the shared transaction (history, `--dry-run`, `--if-revision`).

Plan templates are stored in `paint_plans` as `{plan, plan_hash}`; `plan-show`
writes one for `paint-plan`, which remains the only way to run it.

`pentool paint-diff BEFORE AFTER` is read-only. For documents it reports
added/removed/changed names per section, per-template plan diffs, and per raster
layer the changed tile count, tile bounds and tile-map hashes. With `--plans` it
compares two plans: added, removed, changed (by field) and reordered passes plus
limit and header changes.

Package manifests carry a `paint_assets` table (`section`, `id`, `hash`) per asset;
`pack`, `collect` and `verify` recompute it and reject a mismatch.

## Studio endpoints (engine 1 implementation)

`POST /api/paint/plan` takes `{document, page?, plan, dry_run?, expect_plan_hash?}`
and returns `{result, dry_run, document?}`; `result` is byte-identical to
`paint-plan`. `dry_run` omits the document. A stale `expect_plan_hash` is
`[plan-changed]` and nothing is returned. `POST /api/paint/score` takes
`{before, after, layer, goal}` and returns the `paint-score` report. Both are
stateless and read nothing from disk.

## Conformance and recovery (engine 1 implementation)

`tests/v0130_conformance.rs` pins the canonical plan hash and tile-map digest of a
reference plan; both come from the integer pixel path, so they must match on every
platform and a mismatch is a conformance break, never a golden to refresh. The
same file mutates plans with 300 deterministic junk values (null, extremes, NUL
strings, wrong types), feeds hostile layer dimensions, and asserts that a rejected
plan, a locked layer or a missing target leaves the document unchanged. Plan
limits are hard ceilings: more than 64 passes or a raised `limits` value is
rejected. A long session (64-pass plans repeated) keeps one tile and embeds no
per-stroke history; the full 10,048-stroke run is
`cargo test --release --test v0130_conformance -- --ignored`. Dry run, stale
revisions and plan-hash guards are covered in `tests/v0130_plan.rs`, and crash
safety comes from the shared atomic transaction path.

## Material paint modes (engine 1 implementation)

A brush may carry a `media` object (`brush.media`, valid with blend `normal`
only). It selects one documented, bounded pixel recipe; there is no external
texture file, so the "texture assets" are procedural functions of
`(x, y, seed, scale)` and the recipe is content-addressed by
`pentool paint-media describe` (`digest` = sha256 over the algorithm version, the
canonical parameters and a 64x64 grain sample). Journaled brushes carry the
canonical form including `"algorithm": 1`; an unknown algorithm fails with
`[unsupported-capability]` instead of replaying differently.

Kinds: `glaze`, `dry-brush`, `bristle`, `impasto`, `palette-knife`,
`watercolor-wash`, `charcoal`, `pencil`, `canvas-grain`, `paper-grain`.
Parameters a kind does not read are rejected with `[invalid-brush]`, never ignored.

Common: `strength` (0-1), `zones` (any of `shadow`, `midtone`, `highlight`, weighted
by destination luma: shadow below 128 ramping out, highlight above), `preserve_luminance`,
`preserve_chroma` (0-1, pull the result's value / chroma toward the destination),
`pickup` (destination colour mixed into the deposit), `deposit` (scales alpha),
`seed`, `vary` (mix the stroke seed into the grain). Per kind: `scale` (grain
cell, whole px), `direction` (bristle lines), `light` (degrees; relief is lit when
the surface rises toward it), `wetness`, `wet_length` (px over which wetness
decays to 0, measured along the stroke) and `edge` (pooling / ridge strength).

Math: per touched pixel the dab coverage and a canvas-fixed grain value give an
alpha; zones, deposit, kind colour (multiply for glaze/wash, relief light for
impasto/knife), pickup and preserve controls then run in integer arithmetic. Grain
is a splitmix-style integer hash and integer value noise, so results are identical
on every platform and replay exactly (`raster verify --replay`).

## Visual analysis (engine 1 implementation)

`pentool paint-analyze DOC [--page ID] --scope page|node:ID|layer:ID|group:ID
[--rect x,y,w,h | --selection JSON] [--features LIST] [--max-edge N]` is read-only
and never dumps pixels. The scope is rendered, cropped, reduced to at most
`--max-edge` (default 256, max 1024) pixels with the integer box filter, and
analysed with integer arithmetic only. Every response carries `algorithm`,
`renderer`, `page`, `scope`, `revision` (document revision), `origin`, `source` and
`analysis` sizes, `source_hash` and `analysis_hash`. Coordinates are scope pixels.

Features (`--features`, default all): `contours` (thinned Sobel edges, chained and
Douglas-Peucker simplified into polylines with bounds/strength), `silhouettes`
(connected opaque regions), `values` (2-8 luma zones with share, bounds, mean and a
16-bin histogram), `palette` (dominant colours, 4-bit bins), `texture` (Laplacian
bins and a smooth/moderate/busy label), `orientation` (gradient histogram,
`suggested_stroke_degrees` perpendicular to the dominant gradient, plus a 4x4 grid of
per-cell suggestions), `focal` (top 8 of an 8x8 grid by edge density and contrast),
`gaps` (coverage and empty regions) and `clipping` (shadow/highlight clipping shares).
List features page with `matches`, `returned`, `offset`, `limit` (default 50, max
500) and `has_more` exactly describing the emitted items.

## Cancellation

Cancellation is cooperative and checked before and after each of these bounded
stages: parse, resolve each dependency, rasterize each constraint, generate each
field row, flatten each path segment, generate each 1,024 samples, compile each
pass, render each tile, encode each artifact, and before transaction commit.
Codecs and existing raster primitives retain their documented bounded internal
boundaries. Cancellation after the final pre-commit check cannot interrupt the
atomic file replacement. A cancelled response uses `cancelled`, names the last
completed stage, and reports no committed work.

## Hard limits

Implementations may accept smaller user budgets, but never larger values than this
table. Limits are checked before allocation or pixel work and aggregate over a plan.

| Resource | Protocol maximum |
| --- | ---: |
| request JSON | 4 MiB |
| path drawing segments / request | 4,096 |
| property curves / path | 8 |
| points / property curve | 64 |
| flattened points / path | 262,144 |
| generated samples / path | 262,144 |
| generated dabs / pass | 1,000,000 |
| generated dabs / plan | 4,000,000 |
| passes / plan | 64 |
| regions or constraints / pass | 64 |
| protected `avoid` regions / pass | 32 |
| palette entries | 256 |
| field width or height | 4,096 cells |
| field cells | 4,194,304 |
| resolved pixels / scope | 67,108,864 |
| changed pixels / plan | 67,108,864 |
| tiles touched / plan | 4,096 |
| preview or heat-map pixels | 4,194,304 |
| analysis records returned | 10,000 |
| artifact bytes / result | 64 MiB |
| warnings / result | 256 |

Counts use checked 64-bit arithmetic. Dimensions are validated before multiplication.
Output pagination defaults to 100 and cannot exceed 1,000 records per page. Time is
not a reproducibility input; a caller deadline only requests cancellation.

## Stable results and errors

Results use the result schema and stable snake-case keys. `status` is `planned`,
`committed`, `empty`, or `error`. Counts describe emitted or committed work exactly;
estimates are separately labeled. Bounds are `[x,y,width,height]`, use document
coordinates, and are `null` for no changed pixels. Ratios include a `numerator`,
`denominator`, and `value`, with `value: null` when the denominator is zero.

Errors have a stable code, stage, message, and optional object identifiers and
limit details. Required codes are `invalid-request`, `invalid-path`,
`invalid-curve`, `missing-resource`, `dependency-changed`, `unsupported-capability`,
`limit-exceeded`, `locked`, `conflict`, `cancelled`, `corrupt-raster`, and
`internal-error`. Messages are diagnostic, not machine identity. Failed results
must not claim changed bounds, committed tiles, or a new revision.

## Threat model

Inputs are hostile. Implementations must defend against decompression bombs,
oversized dimensions and products, deeply nested JSON, duplicate IDs, non-finite
numbers, path subdivision explosions, tiny spacing, huge brushes, field and mask
allocation attacks, hash confusion, traversal in artifact paths, symlink escape,
malformed images/profiles, stale revisions, lock bypass, cancellation races, cache
poisoning, and attempts to smuggle executable or remote behavior through presets.

Protocol data cannot contain URLs, filesystem paths, environment expansion,
commands, shaders, scripts, prompts to execute, or provider credentials. External
references must first become ordinary validated, content-addressed Pentool assets.
Artifact paths are output names chosen by the caller and are subject to the existing
path-containment rules; they are never read from a plan. Derived caches are
untrusted: every cache hit is verified against its complete key and content hash.

Memory, disk, output, and work estimates are checked with conservative upper bounds.
The transaction stages new data in a contained temporary location and publishes it
only after validation. Logs and errors do not include image bytes, secrets, full
pixel planes, or unbounded protocol input.

## Conformance fixtures

[`fixtures/agent-painting-v1-manifest.json`](fixtures/agent-painting-v1-manifest.json)
classifies all fixtures. Valid fixtures must satisfy their schema and semantic
rules. Invalid and adversarial fixtures must fail with the listed stable error
before mutation. The cross-platform vector fixes normalized input and expected
canonical samples; later implementation work must add expected dabs, tile hashes,
journal bytes, and rendered pixels before mutation commands can be considered
conformant.
