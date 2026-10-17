# v0.17.0 — Design interchange

Bring work in from, and send work out to, the formats designers already use,
without losing structure where the target format can hold it and without
silently losing anything where it cannot.

## Product contract

- Every import and export produces a **fidelity report**: what mapped exactly,
  what was approximated, and what was dropped, each with the source object ID.
  There are no silent drops.
- Import is bounded and hostile-input safe (archive paths, decompression ratio,
  node count, nesting, image size) and never contacts the network.
- Imports create ordinary `.pen` nodes with stable generated IDs; they do not
  introduce a parallel model.
- Round trips are tested: an exported-then-reimported document keeps the
  properties the fidelity table says are preserved.
- No proprietary SDKs and no system libraries are linked. Parsers are Pentool's
  own Rust code or small audited pure Rust dependencies.

## Formats, in priority order

| Format | Import | Export | Notes |
|---|---|---|---|
| SVG | documented bounded static profile | existing | supported style, geometry, paint, clip, mask, and text features are listed in the fidelity guide; unsupported content is reported or rejected |
| PDF (vector) | documented bounded page, path, text, and image profile | existing PDF | supported operators, fonts, and image filters are listed in the fidelity guide |
| PSD | documented bounded layer, mask, and adjustment profile | flattened + layers subset | 8-bit raster profile and supported adjustments are listed in the fidelity guide |
| Figma (`.fig` / JSON REST export) | documented REST JSON and native v15 profiles | none | fixed-size auto-layout placement and component snapshots; no live reflow |
| Sketch (`.sketch`) | documented bounded artboard, symbol, and style profile | none | unsupported overrides and styles are reported |
| Adobe Illustrator (`.ai`, PDF-compatible) | via the PDF path | via PDF | native `.ai` internals are out of scope |
| IDML | per v0.14.0 | per v0.14.0 | reuse, do not duplicate |

## v0.17 release scope decision

The initial phrase “full static subset” did not define a testable SVG or vendor
format profile, and the initial notes also implied broader PDF, PSD, Figma, and
Sketch fidelity than could be guaranteed by the bounded parsers. For this
milestone, the release contract is the explicit supported profile and limits in
[`docs/design-interchange-v1.md`](../../design-interchange-v1.md). Every feature
outside that profile must be identified in the fidelity report or fail import;
the release must not claim full vendor-format compatibility. Text-to-outline,
responsive Figma reflow, and broader SVG/PDF/PSD/Sketch semantics are deferred.
This scope clarification does not change `.pen` format compatibility.

## Must ship, in order

- [ ] **1. Fidelity report schema** and a shared import harness with limits.
- [ ] **2. SVG import hardening** to the documented bounded static profile,
  with a deterministic supported and hostile-input conformance corpus.
- [ ] **3. PDF vector import** with font and image extraction rules.
- [ ] **4. PSD import** reusing the v0.13.0 layer model.
- [ ] **5. Figma export-file import** with auto-layout mapped to Pentool layout.
- [ ] **6. Sketch import** with symbols mapped to components.
- [ ] **7. Round-trip tests** and a documented fidelity table per format.
- [ ] **8. CLI and editor entry points,** with a fidelity-report viewer.
- [ ] **9. Security and size evidence,** including hostile archives.

## CLI direction

```sh
pentool import DOC design.fig --report report.json
pentool import DOC icons.svg --prefix icons
pentool export DOC out.psd --layers
```

## Explicitly deferred

- Live sync with Figma/Sketch cloud services, plugins that call vendor SDKs, and
  proprietary prototype or interaction data.

## Implementation status (in progress)

The current working implementation is a partial foundation, not a completed
0.17.0 milestone: bounded fidelity reports, static SVG import, Sketch ZIP page
and symbol snapshots, saved Figma REST JSON import with component snapshots and
one-time fixed-size auto-layout placement (including wrapped rows), bounded classic-xref and uncompressed-object xref-stream PDF import, and a
bounded PSD v1 raster-layer import are implemented. Binary Figma `.fig` import
is implemented for the unwrapped `fig-kiwi` v15 Deflate container only, using
an owned decoder with explicit decompression, recursion, and allocation limits.
ZIP-wrapped, Zstandard, FigJam, and other `.fig` versions remain unsupported.
Figma REST JSON remains supported as a separate input. Regular Figma polygons
and stars map to editable closed paths with point-count limits; rounded corners
are approximated and reported. These importers explicitly report or reject
features outside their supported subsets. Figma text component properties ending
in `#text` apply to a uniquely named materialized fallback node; ambiguous,
unsupported, and over-limit properties are reported. Live override propagation
and responsive reflow remain unsupported. Plain/FlateDecode xref streams support
uncompressed objects and bounded type-2 references to scalar, array, and
dictionary values inside plain/FlateDecode object streams. Object-stream members
containing streams, nested object streams, and hybrid/incremental revisions
remain unsupported. PDF image XObjects also support baseline JPEG
DeviceRGB/DeviceGray with decoder dimensions checked against the PDF dictionary.
Page rotation in multiples of 90 degrees, including inherited page-tree rotation,
is applied to vector and image coordinates. Rotated text glyph orientation remains
an explicit approximation and is identified in the page fidelity report.
PDF text extraction is limited to simple one-byte `Tj` runs and `TJ` arrays
containing mapped strings and numeric character adjustments, with bounded
`bfchar` and sequential `bfrange` mappings for permitted embedded TrueType
fonts. PDF path strokes preserve the bounded `M` miter-limit operator and its
default value of 10. Embedded images support FlateDecode 8-bit RGB/gray/CMYK, PNG predictors
10–15 for matching 8-bit image rows, packed 1-bit gray, and baseline JPEG
XObjects. DeviceCMYK vector paints convert to sRGB using a simple device
formula and are reported as approximations; calibrated colors and output
intents are not applied. Matching 8-bit DeviceGray FlateDecode image soft masks
are applied as alpha. Explicit `/Mask` color-key arrays are supported for 8-bit
DeviceGray/DeviceRGB images as binary transparency; image-mask streams, other
color spaces/bit depths, malformed ranges, other soft-mask variants, inline
images, and newer PDF structures remain unsupported.
PSD folder hierarchy and 8-bit raster masks with bounded rectangles are
supported when their flags contain only relative-position and/or invert bits. Legacy RGB
Brightness/Contrast blocks with neutral mean and no Lab-only mode, plus empty
Invert adjustment blocks, map to native v6 scoped adjustments; other adjustment kinds, masked/clipped adjustments, and
non-neutral legacy semantics remain unsupported. Flattened and named full-canvas raster-layer PSD
export are implemented; neither retains vector editability or profiles. SVG basic text
objects remain editable with inherited basic typography; styled `<tspan>` runs
and advanced text positioning are explicitly approximated. Object-bounding-box
linear and radial SVG gradients with solid stops import as native v12 shape
paints and survive SVG export and CPU rendering. One untransformed rectangular
clip in `userSpaceOnUse` or `objectBoundingBox` coordinates can be applied to an
untransformed, unrounded, fill-only rectangle. Solid fill/stroke opacity,
static hidden visibility, and SVG `stroke-miterlimit` from attributes, CSS rules,
inline declarations, and inherited styles are preserved for the supported native
range 1 through 1000; invalid values are rejected. Simple SVG alpha and luminance masks
with one plain rect, explicit matching `userSpaceOnUse` mask/content bounds,
and no stroke map to reusable vector mask resources; luminance uses linear-light
sRGB. Supported mask resources are capped at 256, with no external resolution
or document-wide raster fallback. Group/object-bounding-box/transformed masks,
multi-shape masks, and masks on other geometry remain unsupported. Other clip
paths remain unsupported. SVG input bytes are
checked before XML parsing and its DOM has a bounded node count. Local SVG
`<use>` references to primitive shapes, static groups, and `<symbol>` elements
with a valid `viewBox` are expanded into editable geometry with unique IDs and
bounded by nesting and node limits. Symbol instance dimensions and
`preserveAspectRatio` alignment are applied as transforms; symbols without a
viewBox remain unsupported and reported. Text
gradient rendering/outlining and other SVG gradient variants also remain
incomplete. Sketch layer opacity and common shared-style fill/border opacity are
preserved. Normal-blend Sketch outer shadows with finite offsets, blur up to 256,
valid color, and zero spread map to native drop-shadow effects; unsupported
shadow parameters are reported. Shared Sketch text-style alignment values for
left, right, and center map to Pentool text alignment; unsupported alignment
enums and paragraph spacing, indents, tabs, and line height remain reported.
Sketch symbol masters become component snapshots; instances without
materialized layers use a known master snapshot as fallback with regenerated
node IDs and an aggregate node-count limit. Override values, shared-style
effects, and non-normal paint blend modes have incomplete or dropped fidelity.
Empty Sketch symbol instances now hydrate from a known cross-page master when
available; copied node IDs are regenerated and total expansion is bounded by
the import node limit and a 256-level nesting cap. Uniquely matched
`_stringValue` overrides apply to materialized or master-derived text fallbacks
when the original layer object ID resolves uniquely; copied Pentool IDs remain
regenerated. Ambiguous matches and unsupported override types remain unapplied
and reported.
PNG/SVG/PDF exports emit bounded fidelity reports. Focused `.pen` persistence
round trips cover Figma and Sketch imports, while SVG gradients and symbols, PDF
paths, and PSD embedded raster assets have export/re-import coverage. The current
SVG conformance corpus is deterministic and covers representative supported and
hostile cases. The scope decision above makes the fidelity guide the testable
format contract for this release. Local Windows verification has passed the full
locked all-target suite and Clippy. Three-platform CI, including hostile archive
coverage and release packaging smoke tests, remains a release gate; do not check
acceptance boxes or tag until it passes on the exact commit.
