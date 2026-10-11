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
