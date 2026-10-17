# Design interchange fidelity and resource limits

This document defines the shared v0.17 interchange report used by new static
format importers. It does not change the existing IDML report contract.

## Fidelity report

Every source object considered by an importer or exporter is recorded as
`exact`, `approximated`, or `dropped`. Each detail includes the source object's
stable identifier. Document-level facts without a source object use
`$document`; source files with no stable IDs use a deterministic generated ID.
Counts include every recorded fact even when the detail list reaches its cap.
The machine-readable Draft 2020-12 contract is
[`design-interchange-v1.schema.json`](design-interchange-v1.schema.json).

```json
{
  "schema_version": 1,
  "format": "svg",
  "source": "icons.svg",
  "counts": {"exact": 2, "approximated": 0, "dropped": 1},
  "items": [
    {"source_id":"mark-1", "feature":"geometry", "status":"exact", "message":"Path imported"},
    {"source_id":"shadow-1", "feature":"filter", "status":"dropped", "message":"Filter is unsupported", "action":"Rasterize before import"}
  ],
  "items_omitted": 0
}
```

`items` is capped at 5,000 by default. Consumers must use `counts` and
`items_omitted` rather than assuming the detail array is exhaustive. Reports
are deterministic for the same input and importer version.

CLI exports to PNG, SVG, PDF, and flattened PSD emit the counts in their JSON result. Pass
`--report PATH` to retain the complete bounded per-object report. PNG reports
mark scene objects approximated because the target is flattened pixels; vector
exports distinguish supported geometry, text/font approximations, and raster or
component-fallback content.

PSD export defaults to a flattened 8-bit RGBA composite with no embedded
profile. With `--layers`, it writes one named full-canvas raster layer per source
layer and retains visibility; vector editability and profiles are not retained.
Every exported source object is reported as approximated. Export checks the
fully serialized size, including variable-length layer records and padded names,
against its output limit before allocating the PSD sections.

PSD import retains ordinary raster layers and folders. It applies 8-bit raster
layer masks with zero flags, relative-position, invert, or combined relative and
invert flags, including mask rectangles offset from or smaller than the layer
bounds and inverted default alpha outside those bounds. At the root level, clipped raster layers
are alpha-clipped to the immediately preceding base layer. Nested clipping
relationships, vector masks, adjustment layers, and other mask flag combinations
remain unsupported and appear in the fidelity report. Mask bounds outside the
supported dimension/pixel limits, or with unknown flag bits, are also rejected
as unsupported rather than wrapped or sampled with altered semantics.

CLI reports are scoped to the pages actually exported. Hidden objects are
reported as dropped, and outputs that rasterize a compositing scene identify
all source objects as approximated rather than claiming vector preservation.
Focused integration tests serialize imported Figma and Sketch scenes to ordinary
`.pen` JSON, reopen and validate them, and check representative component,
override, shared-style, shadow, and text properties. SVG, PDF, and PSD export
round trips are also covered for their supported vector-gradient, path, and
embedded-raster behavior; PSD export is intentionally flattened by default.

The current SVG importer preserves basic `<text>` as editable Pentool text,
including inherited font family, size, weight, italic style, fill, opacity,
letter spacing, text anchor, baseline position, and transform. Nested `<tspan>`
content is retained as one run, but per-run styling and positioning are reported
as approximated. Solid paint opacity is applied to shape fill/stroke paints and
editable text fill. Static `display:none` and inherited `visibility:hidden`
retain their nodes with hidden state, including a visible child override beneath
a visibility-hidden group. SVG object-bounding-box linear and radial gradients with solid
color stops import as native v12 shape paints and survive SVG export and CPU
rendering. Local same-kind gradient `href`/`xlink:href` chains inherit omitted
coordinates, spread/color-space settings, transforms, and stops; child attributes
override inherited attributes, and child stops replace inherited stops. Cycles,
missing references, cross-kind references, and chains deeper than 256 are reported
as unsupported without recursive expansion. A single untransformed rectangular clip in either
`userSpaceOnUse` or `objectBoundingBox` coordinates can be applied to a
top-level, untransformed, unrounded, fill-only `<rect>` by intersecting its
bounds. Other clip paths remain unsupported and are reported as dropped. A
static SVG mask subset is supported for a top-level rectangle:
the definition must contain one plain rectangle, use explicit matching
`userSpaceOnUse` bounds and content units, and use a solid fill with no stroke.
Alpha and luminance mask types are mapped to a reusable vector mask resource;
luminance uses linear-light sRGB. The importer caps supported mask resources at
256 and does not fetch references or rasterize the document. Group masks,
object-bounding-box masks, transformed or multi-shape masks, filters, and masks
on other geometry remain dropped. Gradients on editable text, `userSpaceOnUse`,
linked gradient definitions, focal radial points, text outlining, and full CSS
stylesheets also remain unsupported or approximated.

SVG `stroke-miterlimit` is retained from presentation attributes, stylesheet rules,
and inline styles for shapes, with inherited values and the Pentool-supported
range 1 through 1000. Invalid values fail import instead of silently changing
stroke joins.

Local `<use>` references to a primitive (`rect`, `circle`, `ellipse`, `line`,
`polyline`, `polygon`, or `path`) or a static group are expanded into ordinary
editable geometry. The importer preserves group and instance transforms, applies
inherited presentation styles, and generates unique node IDs for each instance.
Cyclic references fail safely; expansion is bounded by the shared nesting and
node limits. Local `<use>` references to `<symbol>` are expanded too when the
symbol has a valid `viewBox`; instance `x`, `y`, `width`, `height`, and the
symbol's `preserveAspectRatio` (`meet`, `slice`, or `none` with valid alignment)
map to an editable group transform. Symbols without a `viewBox` fail import
because their viewport size cannot be determined reliably. Group masks and
clips are reported as dropped. Missing targets and other element types are
reported as dropped.

PDF-compatible Illustrator `.ai` files are routed through the same PDF importer,
and `.ai` export writes the existing PDF output under the Illustrator extension;
native Illustrator internals are not parsed or emitted. The PDF importer accepts
baseline JPEG DeviceRGB/DeviceGray XObjects in addition
to FlateDecode images. It checks the JPEG decoder's dimensions and color channels
against the PDF image dictionary under decoder allocation limits, then embeds
the samples as PNG scene assets. JPEG import is approximated because PDF device
calibration and output intent are not applied. Matching 8-bit DeviceGray
FlateDecode soft masks are applied to image alpha when they have the same
dimensions as their base image and contain no nested mask, explicit Decode
array, or Matte value. Other soft masks are reported as dropped and the base
image is retained without transparency. Explicit `/Mask` color-key arrays are
supported for 8-bit DeviceGray and DeviceRGB images as binary transparency;
image-mask streams, other color spaces/bit depths, and malformed ranges are
reported as dropped and leave the base image opaque.
Page rotation in multiples of 90 degrees is applied to vector and image
coordinates, including inherited page-tree rotation. Rotated text glyphs are
not rotated with the page; each such page receives an approximation report.
PDF stroke state includes the `M` miter-limit operator, preserving its value on
imported paths; the PDF graphics-state default of 10 is retained when the
operator is absent. Values outside the bounded 1..=1,000,000 range are rejected.

## Default import ceilings

| Resource | Limit |
|---|---:|
| Input file | 512 MiB |
| Archive entries | 20,000 |
| Total declared expanded archive size | 2 GiB |
| Per-entry compression ratio | 100:1 |
| Imported node count | 1,000,000 |
| Nesting depth | 128 (format validation may set a lower limit) |
| Encoded image payload | 64 MiB |

Archive entry paths are validated before decompression. Absolute paths, drive
prefixes, and parent traversal are refused. Importers read selected members in
memory; they do not extract archives to disk or resolve external resources.
Format-specific limits may be stricter and must be applied before expensive
parsing or allocation.

## Compatibility

The report is additive metadata and is not embedded into imported nodes. The
imported result remains an ordinary editable `.pen` document. Existing IDML
imports keep their current `supported`/`approximated`/`carried`/`unsupported`
report vocabulary until a separately versioned migration is approved.

## Format fidelity summary

| Format | Preserved by the current importer/exporter | Approximated or reported as dropped |
|---|---|---|
| SVG | Static paths and basic shapes, transforms, basic editable text, supported CSS, solid paints, supported shape gradients, solid opacity, hidden visibility, primitive, static-group, and viewBox-backed symbol local `<use>`, one limited rectangular clip, and one limited static rectangular mask | Styled text runs, advanced CSS/text positioning, general clips/masks, filters, and unsupported gradient variants |
| PDF | Classic-xref tables or plain/FlateDecode xref streams, including bounded type-2 entries for non-stream scalar, array, and dictionary values inside plain/FlateDecode object streams; page rotation, supported vector paths, mapped text using permitted embedded TrueType fonts, bounded Flate/JPEG images, PNG predictors 10–15, matching 8-bit FlateDecode DeviceGray soft masks, 8-bit DeviceGray/DeviceRGB color-key masks, and DeviceCMYK vector/image samples converted to sRGB | Object-stream members containing streams, nested object streams, hybrid/incremental xrefs, encryption, rotated text glyphs, transparency groups, image-mask streams and unsupported soft-mask variants, calibrated color, JPEG DeviceCMYK, inline images, and unsupported text encodings. DeviceCMYK conversion is a reported simple approximation |
| PSD | 8-bit RGB/grayscale raster layers, names/order/visibility, folders, bounded raster masks with relative-position and/or invert flags, root clipping alpha, legacy RGB Brightness/Contrast adjustments with neutral mean, and empty-payload Invert adjustments | Profiles and vector editability; other adjustment/effect layers, masked or clipped adjustments, nested clipping, vector masks, unsupported blend modes, non-neutral legacy adjustment semantics, and other mask layouts |
| Figma | Saved REST `DOCUMENT` JSON and legacy unwrapped `.fig` v15; frames, rectangles, ellipses, lines, regular polygons, stars, text, component snapshots, instances, uniquely name-matched text component properties in materialized fallbacks, and one-time fixed-size auto-layout placement including horizontal/vertical wrapping | Responsive reflow/resizing, FILL/HUG behavior, baseline alignment, live override propagation, ambiguous/unmatched and non-text component properties, embedded blobs, arbitrary vector/boolean nodes, rounded polygon/star corners, and ZIP/Zstandard/other `.fig` versions |
| Sketch | Static artboards and supported shapes/text, shared fill/border opacity, simple normal-blend outer and inner shadows with zero spread, left/center/right text alignment, symbol component snapshots, materialized instance appearance, and uniquely matched string overrides on materialized or master-derived text fallbacks | Ambiguous overrides, style/color/image overrides, nonzero shadow spread, paragraph spacing/indents/tabs/line height, unsupported layer classes, and unsupported blend modes |

Fidelity reports identify source objects for every supported or omitted feature
the importer examines. The table describes only the implemented subset; it is
not a claim of full format compatibility.

For Figma instances, a bounded `TEXT` component property whose key ends in
`#text` is applied to exactly one materialized fallback text node with the
matching name. Ambiguous, unmatched, non-text, or over-limit properties are
reported as dropped. This captures the current text appearance; Pentool does not
retain a live Figma override binding.

Sketch normal-blend outer and inner shadows with finite offsets, blur up to 256,
a valid color, and zero spread map to the native ordered `drop-shadow` and
`inner-shadow` effect stack respectively. Local and resolved shared layer styles
use the same mapping. Other shadow parameters are reported as dropped; disabled
shadows are ignored. Sketch symbol masters become component snapshots. When an instance archive omits
materialized children, the importer hydrates its fallback from a known master,
regenerates copied node IDs from the instance and source identity, and bounds
aggregate fallback expansion by the import node limit and 256-level nesting cap.
For materialized fallback children, `_stringValue` overrides apply only when the
override's exact original object ID resolves to one text layer. Master-derived
fallbacks retain original IDs in private import metadata while their Pentool IDs
are regenerated. Ambiguous matches, other override forms, and unsupported style
effects remain reported and are not applied.

### Native Figma `.fig` subset

Native binary import currently accepts only an unwrapped `fig-kiwi` version 15
file with Deflate-compressed schema and message chunks. ZIP-wrapped files,
FigJam containers, Zstandard chunks, and other file versions are rejected with
`unsupported-format`. The reader does not extract archives or images. It maps
the DOCUMENT/CANVAS hierarchy, frames/groups, rectangles, ellipses, lines,
text, regular polygons, stars, and component snapshots/instances through the
same importer used for Figma JSON. Bounded point counts become editable closed
paths; rounded corners are reported as approximated. Arbitrary vector/boolean
geometry and embedded blobs are named
in the fidelity report; they are not silently treated as imported geometry.

This format subset has stricter limits than the shared defaults: 32 MiB input,
8 MiB decoded schema, 64 MiB decoded message, 500,000 decoded values, 16 MiB
of strings, 250,000 nodes/array items, and 64 levels of Kiwi/tree nesting.
Framing accepts at most 32 non-empty, in-bounds chunks and requires at least
the schema and message chunks. Deflate output is capped while streaming, before
the decoded buffer can grow past its schema/message limit. Invalid lengths,
truncated streams, malformed UTF-8/varints/schema references, duplicate or
unknown message fields, trailing nonzero message bytes, excessive array counts,
allocation estimates, or nesting depth fail the import with an error; no partial
document is returned. Array slot memory is budgeted before backing storage is
reserved. These checks bound decoding but do not make unsupported payloads
importable: embedded blobs remain fidelity-report entries.
The included v15 regression file is attributed in
[`figma-weather-now-v15.NOTICE.md`](fixtures/figma-weather-now-v15.NOTICE.md).
