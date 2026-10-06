# Compositing development contract

Status: work in progress, not a shipped release or a completion claim for TASKS.md.
The implementation spans the shared compositing pipeline, CLI workflows, portable
appearances, batch operations and editor controls. Hosted cross-platform acceptance
remain release gates. The reproducible photographic campaign is in
[`examples/v080-campaign`](../../../examples/v080-campaign/README.md). Document format v6 and
compositing engine 1 explicitly extend v5 without changing its released contract.
The machine schema is [pen-format-v6.schema.json](../../pen-format-v6.schema.json).
Data-only appearance packages use [penpreset.schema.json](../../penpreset.schema.json).

## Compatibility and mutations

`pentool migrate document.pen --target 6` adds `compositing` with `engine: 1` and
`color_space: "srgb8"`. Existing image source records, IDs, nested ordering and
extension data survive. Adjustment commands require this explicit migration.
Migration back to v5/v4/v3 fails when the destination cannot represent required
nodes; it never discards adjustments or sources. Empty unextended v6 documents
can migrate back to v5 and, when they have no images, v4.

Every CLI mutation uses the shared revision guard and history transaction. Library
adjustment planners also validate a candidate before replacing their input.

## Adjustment nodes and scope

An adjustment has `kind: "adjustment"`, stable `id`, `adjustment`, `params`, `scope`,
boolean `enabled`, `opacity` in 0..1 (default 1), and `blend_mode` (initially
`normal` by default). Ordinary node extension fields survive edits. Each page admits at most
64 adjustment nodes; parameters are validated even when a node is disabled.

Nodes evaluate back-to-front. `scope: {"kind":"below"}` grades the composited
preceding siblings in its own layer or group, excluding the page background and
other layers. It has no effect on later siblings. Groups initially isolate their
children. `{"kind":"group","id":"hero"}` and
`{"kind":"targets","ids":["photo","texture"]}` select the visible alpha
contribution of named preceding siblings in that same stack. They cannot refer to
an adjustment, a future sibling, another page, or descendants in another stack.
Group scope requires a group. Missing references and incorrect kinds are errors.

Target coverage begins at the target's rendered alpha. Each later content sibling
attenuates earlier target coverage by `(1 - sibling_alpha)`. Coverage for multiple
targets is summed and clamped to one. An adjustment computes graded RGB from the
current composite, mixes it using its opacity, then mixes that result through
the scope coverage; composite alpha never changes. Thus fully occluded targets
cannot grade unrelated foreground, and repeated adjustments retain stack order.
Scope does not require retaining one full RGBA surface per sibling.

This coverage interpretation explicitly defines how target scopes interact with
partially transparent overlapping siblings; it is not a claim of Photoshop parity.
Mask attachment further restricts the scope coverage. Adjustment blend modes apply
to the original and fully adjusted color, then opacity and coverage mix that result
with the original. Adjustment alpha remains unchanged.

## Color pipeline

Surfaces contain straight RGBA8 in encoded sRGB. Source-over evaluates with
premultiplied contributions and normalizes RGB by output alpha. Transparent RGB
never contributes. Every stage clamps to 0..255 and rounds half-up. Adjustments
leave alpha intact and skip fully transparent pixels. Opacity mixes the original
and already rounded adjusted channels and rounds again. The engine uses basic
IEEE arithmetic, without platform-dependent transcendental functions.

Exposure here multiplies encoded sRGB by `2^stops`, with stops -16..16; it is not
scene-linear photographic exposure. This is deliberately explicit while linear
compositing and future color management remain separate contracts.

Brightness/contrast, levels, curves and hue/saturation reuse operation engine v1,
including its parameter validation and deterministic gamma approximation. Curves
are piecewise linear, not an unspecified cubic interpolation.

Additional adjustment parameters:

| Adjustment | Parameters and defaults | Evaluation |
| --- | --- | --- |
| exposure | `stops: 0`, -16..16 | RGB multiplied by 2^stops |
| vibrance | `amount: 0`, -100..100 | y + (channel-y) * (1 + amount/100 * (1-saturation)) |
| color-balance | `red`, `green`, `blue`: 0, each -100..100 | Per-channel additive shift of value * 2.55 |
| black-and-white | weights `red: .2126`, `green: .7152`, `blue: .0722`, 0..1, sum 1 | Weighted RGB copied to all channels |
| channel-mixer | Required `matrix`: three rows of `[R,G,B,offset]`, coefficients -2..2 | Dot product plus offset * 255 |
| gradient-map | Required 2..16 strictly ordered `stops`, each `{position,rgb}`; endpoints 0 and 1 | Linear encoded-sRGB interpolation at weighted luminosity / 255 |
| invert | Empty params | 255 - channel |
| posterize | Integer `levels: 2`, 2..256 | Nearest of equally spaced encoded-sRGB channel levels |
| threshold | `level: 128`, 0..255 | Weighted luminosity >= level becomes white, otherwise black |

Weighted luminosity is R*.2126 + G*.7152 + B*.0722. Vibrance saturation is
(max(R,G,B)-min(R,G,B))/255. Gradient RGB channels lie in 0..255. Unknown parameter
keys, malformed arrays, invalid ranges and unknown adjustment kinds are errors.

## Rendering and resource budgets

Reusable masks live in `mask_resources`, keyed by the SHA-256 digest of their
compact canonical JSON. `masks` maps human-facing names to those hashes. Vector
resources snapshot vector content in page coordinates and use its rendered alpha.
Raster resources reference verified image assets; luminosity times source alpha
provides grayscale coverage. Mask creation supports `vector:ID`, `node-alpha:ID`
(a saved grayscale raster snapshot), or `asset:sha256:DIGEST`.

Attachments have a resource hash, invert, density 0..1, feather 0..256, linked
state, optional affine transform and a captured inverse world transform (`anchor`).
Linked masks initially retain their page placement, then follow the owning node's
transform changes. Unlinked masks remain in page space. Inversion precedes feather;
density mixes coverage with white, so density zero reveals everything. Feather uses
the existing premultiplied blur engine at export scale. Nodes and groups multiply
alpha by mask coverage; adjustment masks restrict RGB grading instead. Saved mask
snapshots cannot recursively reference masks or clips. There are at most 256
resources and 256 named aliases. Deleting an alias retains resources used by nodes;
detach removes the attachment. Explicit `mask apply` bakes a node/group's isolated
appearance into a verified PNG source, preserves its ID and page placement,
removes the mask attachment, and retains normal transaction history.

Clipping membership is `clipping: {base: "ID"}`. Members must immediately follow
the base or its existing members in the same sibling stack. Adjustments cannot
interrupt a clipping stack. Members multiply their alpha by the rendered base
alpha, including its mask and opacity. A hidden base conceals the stack. Broken
references or reordering that breaks consecutiveness fail validation with guidance
to detach clipping before reordering. Group stacks are isolated from parent stacks.

Blend modes use the [W3C Compositing Level 1 equations](https://www.w3.org/TR/compositing-1/#blending).
All 16 roadmap modes have exact analytic pixel tests in `v080_compositing.rs`.
These are development conformance fixtures; hosted cross-platform qualification is
still required before shipping. `blend_space` is `srgb` by default or explicitly
`linear`; deterministic sRGB conversion surrounds blend evaluation in linear mode.
Premultiplied source-over includes both non-overlapping contributions and the blend
in the overlapping region. Non-separable modes use the specification's .3/.59/.11
luminosity weights, independently of adjustment luminance. Channel outputs clamp and
round half-up after compositing. Unknown modes or spaces fail validation.

Groups default to `isolation: "isolated"`. A `pass-through` group blends its children
against its parent backdrop. Masked, clipped, translucent or blended groups must
be isolated; invalid combinations fail explicitly. Node opacity scales content
alpha once, including image nodes, before masking and clipping. Content opacity
scales the displayed content, while effect silhouettes use its original alpha.

The v6 compositor borrows the original asset tables and shares the existing
vector/image/text serializer, verified image processing and font renderer.
Geometry is evaluated at requested export scale from immutable sources, rather
than resampling a saved intermediate composite. PNG, embedded-raster SVG and PDF
exports evaluate the same compositor. The document remains editable; exporting
raster-backed SVG does not mutate or bake the document. Live vector SVG for
compositing is not yet implemented, and linked-image or outlined-text export
options fail explicitly for v6. PDF currently contains the composited raster page.

Each temporary-surface budget is 256 MiB; cumulative pixel work is bounded at
1,073,741,824 pixels. These limits include scoped coverage and conservative
adjustment scratch accounting. Canvas axes remain 1..16384. Ordinary export scales
remain 0.1..8; internal v6 proxies may use 1/16384..8, with a minimum one-pixel
output dimension. Unsafe requests fail before allocating a surface or replacing
an output. Source decoder limits are separate from compositor scratch limits;
the 256 MiB scratch budget is not a claim about total process resident memory.
Performance qualification and cross-platform pixels remain task 14 acceptance
work; no release claim is made from Windows-only local tests.

## CLI examples

## Transform, fill, selection and analysis contracts

`transforms` contains at most 64 ordered operations with stable IDs, enabled state,
opacity, optional reusable mask and a 3x3 page-coordinate homography. Fully opaque,
unmasked transforms combine algebraically before sampling. Masked transforms
interpolate inverse coordinates and sample the original surface once with
premultiplied bilinear filtering. Pure affine leaf transforms rasterize from the
original vector/image source. Perspective quads must be ordered, strictly convex,
finite and invertible. No mesh interpolation is specified or implemented.

Fill nodes have a rectangle and a `fill` descriptor: solid, linear, radial, conic,
or pattern. Gradient stops (2..64) have strictly ordered positions with endpoints
0 and 1. Interpolation is premultiplied sRGB by default, optionally linear-light;
spread is pad, repeat or reflect. Coordinates use local canvas units relative to
the fill rectangle's origin, not normalized fractions; linear defaults run 0..1.
Patterns use hash-verified image assets, nearest texels, explicit origin, positive
independent scales and repeat/none modes. Colors support existing token references
and explicit fallback values. Fill transforms are invertible affine matrices.
Complete fill descriptors may use `{ref,fallback}` to reference a named `fill`
style; references resolve with a depth limit of 64 and preserve the descriptor's
portable fallback. Editing one gradient style updates every referencing page.

Effects contain at most 32 ordered, enabled, stable-ID entries. Named effects
styles use `{ref,fallback}`. Stroke distances use a deterministic octagonal metric
(orthogonal distance 1, diagonal distance sqrt(2)); this is not an exact Euclidean
stroke. Blur and feather use the pinned v1 image engine. Radii above 256 output
pixels fail before expensive processing. Shadows use rounded output-pixel offsets.
Drop-shadow/outer-glow modes blend the current content over the underpainted
effect. Blur blends its sampled RGB against the current content before weighted
premultiplied interpolation. Gradient overlays use page-space coordinates;
selection crop retains their coordinate frame through the node's `effect_origin`
offset without changing its shared style. Presets preserve this offset.

Temporary queries accept rectangle, ellipse, polygon/lasso, encoded-sRGB Euclidean
color range, luminosity range, node/mask alpha and saturating add/subtract or min
intersection. Shapes evaluate pixel centers; polygon coverage uses even-odd fill.
Queries are bounded to 128 expressions and nesting 16. `selection query` and
`analyze` never write documents. Save materializes a reusable grayscale mask; crop
wraps retained nodes in a translated masked group and changes only the selected
page. Saved coverage is durable raster data, not a live selection dependency.

Analysis reports 256-bin alpha-weighted channel/luminosity histograms, population
variance, transparency bounds, endpoint clipping counts and at most 1024 samples.
Palette suggestions use deterministic 5-bit RGB buckets and lexical tie-breaking.
Before/after disables reversible enabled operations and masks without rewriting
source assets. Palette tokens require explicit `--palette-tokens PREFIX`.

Linked-source reports inspect actual source files, not cache fallback, so missing
and stale links remain visible. Relink requires identical hashes. Externalize
requires an existing contained parent and never overwrites different bytes.
Collection requires a new output directory, verifies all image assets, rewrites
storage to contained `assets/HASH` paths and publishes the staged directory by
rename. The source document is never modified. Encoded collection size is bounded
to 512 MiB. No network is used.

Data-only appearance presets declare `penpreset` version 1, engine 1 and `srgb8`.
They retain geometry and target IDs and require matching node kinds. Immutable
resources and tokens travel with the preset; conflicting named resources fail
instead of silently changing unrelated content. Sources are embedded and verified.
Preset compatibility is validated before the target is changed. No scripts run.

### Additional CLI examples

```sh
pentool transform add photo.pen portrait scale --params '{"x":2,"y":2}'
pentool fill add photo.pen backdrop --layer content --kind solid --color '#204060'
pentool effect add photo.pen card shadow --x 0 --y 12 --blur 30 --opacity 0.24
pentool selection query photo.pen --query '{"kind":"node-alpha","node":"portrait"}'
pentool selection save photo.pen --name portrait-region --query '{"kind":"node-alpha","node":"portrait"}'
pentool analyze photo.pen --scope group:hero --samples '[[10,20]]' --compare
pentool linked report photo.pen
pentool linked collect photo.pen --path portable-project --dry-run
```

```sh
pentool migrate photo.pen --target 6
pentool adjustment add photo.pen grade --kind curves --scope group:hero --params '{"points":[[0,0],[128,145],[255,255]]}'
pentool adjustment set photo.pen grade --opacity 0.6
pentool adjustment disable photo.pen grade --dry-run
pentool adjustment enable photo.pen grade
pentool adjustment list photo.pen --json
pentool adjustment remove photo.pen grade --if-revision REVISION
pentool mask create photo.pen hero-mask --from vector:hero-shape
pentool mask attach photo.pen grade hero-mask --feather 2
pentool mask detach photo.pen grade
pentool mask apply photo.pen portrait
pentool clip add photo.pen texture --base headline
pentool object photo.pen set texture --layer content --blend overlay --opacity 0.6
```
