# Professional color and print engine 1

Status: implementation contract for the v0.12.0 roadmap, under development.
This document does not claim that the commands or capabilities below have
shipped. The roadmap remains the completion record. Explicit v8 migration and
the profile catalog and native vector paints below are implemented in development.
Native raster interpretation, process/spot plates and production output remain planned. Released RGB rendering
behavior remains authoritative for legacy colors, including migrated scenes.

## Scope and reference standards

The target is offline, reproducible commercial print delivery from an editable
RGB, CMYK, Lab and spot-color scene. The engine is Rust in the existing binary;
opening, inspecting, proofing and rendering a project never fetch dependencies.
ICC scope is ICC.1 v2 and v4, with D50 XYZ or Lab PCS. ICC.2/iccMAX, spectral
processing, proprietary CMM plug-ins and arbitrary executable profile elements
are refused. Device links are explicit recipes, never inferred input profiles.

Reference documents:

- [ICC.1:2022, profile version 4.4](https://www.color.org/specification/ICC.1-2022-05.pdf)
  defines profile storage and transform semantics.
- [ICC v4 specifications](https://www.color.org/v4spec/) provides earlier
  versions and errata; the reader must retain the actual version in diagnostics.
- [PDF Association PDF/X requirements](https://pdfa.org/technical-side-and-requirements-of-pdfx/)
  explains the delivery constraints. The initial delivery target is embedded
  output-intent PDF/X-4 (ISO 15930-7:2010), not external-profile PDF/X-4p.

Conformance requires independent evidence; a header signature, output-intent
dictionary or Pentool's own preflight is insufficient to certify a PDF.

## Profile roles and identity

Input describes stored device samples. Working describes editable process color
and compositing. Proof describes the simulated printing condition. Display
describes the preview destination. Output describes final process separations.
Each role is explicit and may reference the same verified asset. A device link
identifies its input and output channel spaces and is a distinct transform role.
A DCP camera profile remains a photography dependency, not an ICC profile.

The hash-pinned `docs/fixtures/print-v8-roles.pen` exercises explicit working,
display and output targets with an embedded authored RGB monitor probe. Its
unconfigured proof role must fail when requested. Ordinary preview pixels match
`print-v8-native.pen`; this fixture does not represent a printing condition.

Profiles are immutable original bytes addressed as `sha256:<64 lowercase hex>`.
Names are document-local aliases, not identity. Import records byte length, ICC
version, class, channel space, PCS, description, copyright, source and a declared
redistribution policy (`allowed`, `restricted`, `unknown`). No licensing claim is
inferred from a copyright string. Packaging must report and refuse dependencies
whose policy does not allow redistribution unless an explicit local collection
policy permits it. It must never silently omit a profile.

Embedded and contained document-relative external storage reuse the existing
asset rules. Hash and length are verified before use, including after collection.
Absolute paths, parent escapes and symlink escapes are refused. Unknown tags
survive because profile bytes are retained verbatim. Profile import validates
structure and separately reports supported transform capabilities; inspection
must not imply that every structurally readable profile is executable.

Assign changes sample interpretation without changing samples. Convert changes
samples through a selected transform to preserve color appearance. Both require
explicit role, scope and intent, revision guards and the shared transaction
planner. A locked affected object fails the entire edit. Dry run resolves and
verifies exactly the dependencies commit uses. Proof and production export do
not mutate the master.

## ICC execution and limits

Planned executable forms are RGB/gray matrix/TRC and RGB/CMYK/gray LUT profiles,
plus explicitly selected RGB/CMYK device links. Curves include sampled `curv`
and parametric `para` functions 0–4. LUTs include `mft1`, `mft2`, `mAB ` and
`mBA `. A required unsupported element fails with `unsupported-capability`,
including `mpet`; it must never execute an approximate replacement.

All integer offsets, products and byte ranges are checked before allocation or
indexing. Initial engine limits are 16 MiB per profile, 256 tags, 65,536 samples
per curve, four device channels, 65 grid points per CLUT axis and 4,194,304 total
CLUT scalar samples. The tag table and data must fit the declared exact file
length. Tag data are four-byte aligned. Duplicate tag signatures, partial data
overlaps and data overlapping the header/table fail. Different tags may share
exactly the same byte range, as generated RGB TRCs already do. Reserved bytes
and typed payload lengths are validated. Diagnostics include the tag signature
and byte offset where possible.

The public reader distinguishes malformed structure, unsupported versions or
capabilities, and exceeded limits. Inspection returns compact metadata, hashes,
tag signatures and capabilities rather than profile payloads. CLI pagination
uses the same `matches`, `returned`, `offset`, `limit`, `has_more` contract as
other discovery commands. No parser uses unsafe code or a native CMM dependency.

Implemented file discovery uses `color profile inspect FILE --offset N --limit N`
and `color profile validate FILE --direction device-to-pcs|pcs-to-device|device-link
--intent INTENT`. Inspection JSON version 1 reports `icc_version` separately from
the report version. Validation scope is `requested-profile-path`, not certification
of the entire ICC specification. `--page` is invalid for these file commands.
Display metadata accepts `mluc`, legacy `desc` and ASCII `text`. It validates all
localized records and deterministically selects en-US, another English record,
then the first record; it never infers redistribution permission from the text.
Limits are 64 records per metadata tag, 8,192 bytes per string and 65,536 total
referenced string bytes per tag. Future record sizes of at least 12 bytes are
respected. Shared strings are allowed. Invalid Unicode, offsets, reserved bytes
and limits fail rather than being replaced by lossy text.

Device RGB/gray/CMYK samples are finite normalized [0,1] values. User CMYK
percentages are divided by 100 exactly once at the boundary. Lab values use
L* [0,100], a*/b* [-128,127], with ICC v2/v4 LUT encodings decoded explicitly.
PCS XYZ values use Y=1 for reference white and retain extended values between
stages. Alpha is a separate coverage value, never an ink channel or PCS sample.
NaN and infinity fail validation before rendering.

Math uses the existing deterministic photography `f64` kernels and explicit
operation order. Output quantization is round-half-up with saturation at the
declared output range. Sampled curves use linear interpolation. CLUTs use
linear/bilinear interpolation for one/two inputs, tetrahedral interpolation for
three inputs, and linear interpolation between tetrahedral slices along the
first axis for four inputs. The last input axis varies fastest in storage. This
draft choice aligns with the independent LittleCMS reference implementation. Inverse matrix transforms
reject singular or ill-conditioned matrices. Curve inversion requires monotonic
curves, follows ICC Annex F.1 (highest input for an interior plateau, lowest for
a terminal plateau), and uses fixed iteration counts.

Rendering intents are perceptual, media-relative colorimetric, saturation and
ICC-absolute colorimetric. An intent resolves its designated A2B/B2A path before
rendering. A missing LUT path is an error, not an unreported substitution. A
matrix/TRC profile has a single colorimetric model; any equivalence of intent
paths must be reported as such. Absolute colorimetry applies media-white scaling
around the colorimetric PCS path. Chromatic adaptation is applied once, respecting
profile PCS colorants and `chad`, not again to already D50-adapted matrix columns.
Optional colorimetric black-point compensation defaults off; it cannot be
enabled until its algorithm and independent reference vectors are pinned.
Profile linking is a separate operation over these paths. The engine 1 default
follows the LittleCMS 2.18 convention: a v4 destination requests affine XYZ black
mapping for perceptual/saturation. A physical PCS destination requests no extra
mapping. A v4 LUT endpoint uses reference black `[0.00336, 0.0034731, 0.00287]`;
a matrix/TRC endpoint uses its device black. A v2 source LUT uses its selected
intent's darkest colorant (RGB/gray zero, CMYK all one). The detected lightness
is bounded to [0,50], with inverted synthetic values above 95 reset to zero,
retaining chromaticity. The diagonal affine map sends source black to destination
black and D50 white to itself, retaining extended finite PCS until destination
clipping. Equal black points produce no mapping. Source and destination paths,
black points, scale and offset are reported. These are explicit CMM conventions,
not a claim that ICC mandates this linking policy. The independent vectors pin
the chosen behavior. Native Lab/XYZ uses physical PCS endpoints rather than an
inferred device profile. The isolated profile executor does not perform linking;
direct pipeline vectors must not be confused with linked-profile vectors.
The photo engine's existing perceptual RGB gamut mapper is not an ICC LUT.

## Document declarations and migration

Professional color requires an explicitly migrated version 8 document. Old
readers must refuse it instead of ignoring fields that change appearance.
Versions 1–7 retain their released rendering behavior; untagged vector colors
remain sRGB. The new schema must extend the ordered scene graph and existing
asset catalogs, not flatten it or replace the photography catalog.

### Implemented catalog foundation

[`pen-format-v8.schema.json`](pen-format-v8.schema.json) currently describes the
v8 catalog and native paint foundation. It reuses v7 scene and photography
declarations, with native color grammar and canvas backgrounds. Inks and
production geometry remain planned.
`pentool migrate DOCUMENT --target 8` adds this declaration without converting
pixels or color numbers:

```json
"color_management": {
  "engine": 1,
  "legacy_rgb": "srgb",
  "working_space": "srgb:linear",
  "profiles": {},
  "aliases": {}
}
```

These two space declarations are currently fixed. Importing a profile does not
assign it to a color role or change rendering. Versions 1–7 continue treating
`color_management` as an uninterpreted extension. Explicit migration refuses
an existing extension with that name rather than overwriting it.

Profiles are keyed by `sha256:<64 lowercase hex>` of their original bytes.
Records contain `byte_length`, `icc_version`, `class`, `color_space`,
`connection_space`, selected `metadata`, `storage`, `redistribution`, and optional
`source`. Metadata records description, copyright, manufacturer and model,
each as null or `{ "text": "...", "language": null, "country": null }`;
available locale codes replace null. Header fields and metadata must match the
verified original ICC bytes. Aliases match `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`
and point to an existing profile digest.

Embedded storage is `{ "kind": "embedded", "encoding": "base64", "data": "..." }`.
External storage is `{ "kind": "external", "path": "profiles/press.icc" }`.
Paths must remain within the document directory after symlink resolution.
External import verifies an existing file; it does not copy or rewrite it.
Loading and embedding verify its hash, length, header and metadata. Missing files
may use a verified private digest cache; no network lookup occurs. Catalog
validation checks embedded bytes immediately and checks external record structure
without opening external files; loading performs the remaining byte checks.

Limits are 64 profiles, 256 aliases, 16 MiB per ICC file, and 64 MiB total
declared decoded profile bytes. Catalog CLI input and planned serialized document
output are limited to 64 MiB. Selected metadata text is limited to 32,768 UTF-8
bytes per field; source and path text to 4,096 UTF-8 bytes. JSON Schema character
limits are supplemented by these Rust byte limits and semantic checks.
Redistribution is an explicit caller declaration: `allowed`, `restricted`, or
`unknown` (default), never inferred from copyright text. It records provenance;
it does not grant a license.

`color profile import`, `list`, `show` and `embed` share this catalog. Import and
embed use the transaction/history planner, support dry runs and revision guards,
and commit one history entry. Identical bytes share a record; conflicting aliases,
redistribution declarations or provenance fail without mutation. Downgrading
currently refuses any retained profile or alias, even when unused. An empty v8
catalog can downgrade to v7 when no native paint declarations remain, without
changing existing scene/photo declarations. Native declarations also cause
downgrade refusal; a downgrade never silently substitutes preview pixels.

`color profile collect DOCUMENT` verifies every catalog dependency and embeds
its original bytes in one planned transaction. It includes unused profiles,
preserves aliases, metadata and licensing/source declarations, and never omits
a missing or corrupt dependency. Every record must declare `redistribution:
allowed` for sharing. `--local-only` explicitly permits a private collection of
restricted/unknown records; it leaves those policies unchanged. It is not a
redistribution permission or a production package. Limits and the serialized
document size gate also apply to collection. The report records profile identities,
aliases, source facts, total byte length and newly/already embedded counts.

`color profile remove DOCUMENT --name REFERENCE` removes one unused identity
and all its aliases without deleting external files. References in any page,
shared style, component, fallback or mask snapshot reject removal before writing,
including unused shared definitions. Missing external bytes do not prevent
removing an otherwise unused record. Collection and removal support `--dry-run`
and `--if-revision`, commit with the original-byte revision, and share undo/history.

Existing `.penpkg` packaging now audits v8 ICC dependencies. Every catalog entry
must be embedded and declare allowed redistribution; local-only collection does
not bypass this gate. Each asset manifest can carry an `icc_profiles` table with
digest, byte length, ICC version, class/channel/PCS facts, declared policy, source
and aliases. Verification derives the table again from validated original bytes
and rejects missing or mismatched audit entries. Empty tables are omitted for
legacy packages. Manifest-only and discovered assets use the same checks, and
the archive contains the exact validated asset snapshot. Packages remain offline
and deterministic. Private history/cache is excluded from discovery and refused
as an explicit dependency. Preview paths cannot overwrite validated asset or
manifest entries, and included files must resolve within the package root.
Proofs, plates, print recipes and PDF/X validation reports are
still required for the separate production-package milestone.

### Implemented role targets

The optional `color_management.roles` map accepts exactly `working`, `proof`,
`display` and `output`. Each setting contains a native `destination` and explicit
`intent`, for example:

```json
"roles": {
  "working": {
    "destination": { "space": "display-p3:linear" },
    "intent": "relative-colorimetric"
  },
  "proof": {
    "destination": { "space": "icc-cmyk", "profile": "sha256:<original-profile-hash>" },
    "intent": "relative-colorimetric"
  }
}
```

The hash placeholder above must be replaced by an actual catalog identity.
`color role assign DOCUMENT ROLE --to DESTINATION --intent INTENT` verifies
required original bytes and executable paths, then pins an ICC target to its hash.
Working and proof assignments require both device-to-PCS and PCS-to-device paths;
display and output assignments require PCS-to-device. Missing paths fail without
substitution. The report includes the selected path descriptions. Working accepts
native process/PCS targets; display accepts named RGB or an RGB monitor-class ICC;
proof requires an output-class ICC printing condition; output accepts named RGB
or an output-class ICC device profile. Device links require their separate workflow.

Without explicit settings, working resolves to linear sRGB and display to sRGB,
both with relative colorimetry. Proof and output have no implicit printing
condition: a consuming command fails until they are assigned. `color role list`
reports all four effective settings, whether each is configured, and resolved
profile identities. Structural open/validation and listing stay offline and
leave external profile bytes unopened; assignment and consuming operations verify
the dependencies they need. An unused configured display profile does not become
a dependency of the ordinary sRGB preview.

`color role evaluate DOCUMENT ROLE --color SAMPLE` converts a native sample to
the role target with its stored intent and returns channels, alpha, clipping and
selected ICC links. `color paints convert DOCUMENT --role ROLE` consumes that
same target and intent through the shared paint planner. It conflicts with
`--to`, `--legacy-srgb` and an explicit `--intent`; no supplied option is ignored.
The regular paint/page/shared-resource/lock rules apply to this conversion.

Role assignment preserves existing paint channels, profile interpretations,
geometry, extensions and preview pixels. It changes document configuration in
one revision-guarded history transaction; `--dry-run` and undo are supported.
`color role clear DOCUMENT ROLE` removes one explicit setting and retains its
profiles. Missing explicit settings fail clearly. Profile removal refuses any
assigned role dependency, and downgrade requires clearing explicit roles first.
Package verification also compiles each assigned role's required paths from its
embedded original bytes, including role settings written directly in JSON.
Role operations are document-wide and reject `--page`.

Current role consumers are numeric evaluation and explicit paint conversion.
Ordinary exports retain their existing sRGB8 preview policy and fixed engine-1
declarations. Role-aware production compositing, paper/ink proof rendering,
display-tagged proof images and production export remain under development.
Converting a sample to a proof target alone does not simulate viewing a print.

### Implemented native sample evaluation

`color evaluate DOCUMENT --color JSON [--to JSON] --intent INTENT` is a read-only
numeric evaluation command for v8 documents. It shares `src/color/value.rs` with
the scene preview and developing production planners. Its grammar is
[`color-value-v1.schema.json`](color-value-v1.schema.json).

Samples contain `space`, `channels`, optional `profile`, and optional `alpha`
(default 1). Destinations contain only `space` and optional `profile`. Named RGB
spaces are `srgb`, `display-p3`, `adobe-rgb-1998`, `prophoto`, and `rec2020`, each
optionally suffixed `:linear`. Their three channels use [0,1] with the photography
contract's named transfer function, or linear encoding for that suffix.
ICC spaces are `icc-rgb` (three channels), `icc-gray` (one), and `icc-cmyk`
(four in C, M, Y, K order); device values use [0,1] and require a digest or alias
profile reference. The verified ICC header must agree with the declared space.
Device links cannot serve as ordinary source or destination profiles.

`lab-d50` uses physical L*, a*, b* with no profile reference. Extended finite Lab
values are retained when their conversion remains finite; ICC normalized table
encodings are not native Lab values. Alpha is always in [0,1], never enters ICC
channel evaluation, and survives conversion unchanged, including zero alpha.
Unknown sample/destination fields, incorrect counts, invalid numbers and missing
profile references fail explicitly.

Named RGB uses its frozen primaries and source white, then Bradford adaptation
into the exact ICC fixed-point D50 white. ICC PCS is already adapted; it does not
receive another white adaptation. Source-to-destination ICC conversion uses the
shared intent link planner, including its declared black mapping. Missing intent
paths fail rather than substituting another intent. A conversion to the same
space and verified profile identity preserves channel numbers exactly, including
when two aliases identify that profile; its path is reported as `identity`.

Named RGB output clamps to [0,1] and reports gamut clipping beyond a 1e-12
channel roundoff tolerance. ICC output reports its executor's clipping flag;
physical PCS and Lab retain extended values. Each evaluation context loads each
used profile once and caches at most 64 selected ICC links. Reusing a pair does
not consume another slot; exceeding the limit fails before compilation. The CLI
limits color JSON to 4,096 bytes and destination JSON to 256 bytes.

### Implemented native channel interpolation

`color interpolate DOCUMENT --from JSON --to JSON --space JSON --position T
--intent INTENT` is a read-only numeric sampler. `--space` uses the destination
grammar above; T must be finite in [0,1]. Both endpoints convert into that space
once through the shared evaluator, then each channel interpolates as
`a*(1-T)+b*T`. Alpha uses the same weights independently, without premultiplying
channels. Zero-alpha endpoints retain their channel contribution. This specifies
straight interpolation, not compositing.

Named encoded and linear RGB are distinct interpolation spaces. Same-identity
ICC CMYK endpoints interpolate C, M, Y and K directly, without reseparation or
an RGB intermediate; no unused ICC path is required for identity conversion.
Physical Lab interpolation retains extended finite values and validates the
result. Converting other endpoint spaces requires the selected executable ICC
paths and reports clipping before interpolation. The report contains endpoint
conversions and link descriptions, the resulting native sample, position, intent
and verified profile hashes. The operation loads only used profiles once,
retains source documents byte-for-byte and creates no history. CLI samples have
the same 4,096-byte bound and the destination the same 256-byte bound as evaluation.

Explicit native fill gradients use the same channel blend, described below.
The numeric command does not establish process compositing or plate support.

Sample evaluation reports include `xyz_d50`, `source_clipped`, the source ICC link when applicable,
verified `profiles`, and optional converted color, clipping, path and link
description. Evaluation never changes a document, creates history, or assigns
profile roles. Numeric sample conversion does not constitute proofing or PDF/X
conformance evidence.

### Implemented native fill gradients

V8 linear, radial and conic fill definitions accept an explicit destination
object in `space` and require an `intent`. For example, after importing the
authored `print-cmyk-mab-v4.icc` probe as `cmyk-probe`:

```json
{
  "kind": "linear",
  "space": {"space": "icc-cmyk", "profile": "cmyk-probe"},
  "intent": "perceptual",
  "stops": [
    {"position": 0, "color": {"space": "icc-cmyk", "profile": "cmyk-probe", "channels": [1,1,1,1]}},
    {"position": 1, "color": {"space": "icc-cmyk", "profile": "cmyk-probe", "channels": [0,0,0,0]}}
  ]
}
```

This authored input probe is numerical test data, not a press condition. Its
perceptual forward path can preview same-profile CMYK samples; attempting to
convert other spaces into this one-way profile fails because it has no inverse.

Native gradients support the sample destinations above: named RGB and linear
variants, ICC RGB/gray/CMYK and physical D50 Lab. Endpoints convert once using the
declared intent. Their native channels and alpha interpolate separately, then
each subsample converts to sRGB appearance before existing coverage accumulation
and engine-1 compositing. Same-profile device channels remain device channels
until that final appearance conversion. RGB endpoint interpolation is not used
to approximate a nonlinear ICC or Lab ramp. Source and interpolation profiles
must provide every selected path actually consumed; identity conversion does
not require an unused inverse path. All four intents, explicit geometry, pad,
repeat and reflect spread, and gradient overlays use the shared fill renderer.

Missing `space`, or released string values `srgb` and `linear`, retain the
released premultiplied RGB/alpha interpolation. An object `space` explicitly
selects straight native channel interpolation, including the channel contribution
of transparent endpoints. `intent` is accepted only with an object space.
Native destination objects are refused in earlier document versions. This also
closes the old validator's accidental interpretation of an invalid object space
as default sRGB; the released schema already required string spaces.

The private preview clone freezes effective color-token stops before lowering
ordinary paints; stored token bindings, fallbacks, channels and source documents
are unchanged. A render loads the used gradient profile snapshot once and shares
at most four intent contexts, each with the existing 64-link limit. Pixel sampling
reuses prepared fixed-channel buffers and does not allocate JSON or clone link
reports per sample. The existing fill pixel-work/cancellation budget applies;
native gradients additionally permit at most 65,536 total stops per traversal.
Page rendering includes visible nodes and enabled effects; explicit node and mask
analysis resolves only its requested node/resource dependencies on that page.

Interpolation destinations participate in structural/header validation, profile
removal, downgrade refusal and package verification, even when all stops are
legacy colors. Packages compile every declared gradient, including unused shared
definitions, after verifying embedded bytes and redistribution policy. Changes
to a gradient definition use the existing fill editor, locks and transactions.
Converting paint samples does not silently change the interpolation declaration.

The hash-pinned [`fixtures/print-v8-gradient.pen`](fixtures/print-v8-gradient.pen)
has three opaque neutral ramps: linear RGB, physical Lab, and authored CMYK.
`print-conformance.json` pins every preview RGBA byte. RGB and Lab rows also have
independent transfer/CIE pixel assertions; CMYK permits one-byte quantization
relative to the authored neutral Lab probe. The fixture is an sRGB8 preview
golden, not a proof, separation or printing-condition fixture. Production linear
compositing and native plate output remain under development.

### Implemented native scene paints

Native values currently work in canvas backgrounds, node fill/stroke style
fallbacks, named color styles, solid fill nodes, explicit native gradient stops,
and appearance effect colors. The traversal also validates component roots,
instance fallbacks and immutable vector mask snapshots. It follows supported paint fields and their style references;
unrelated extension fields and original image/profile bytes remain opaque.
[`fixtures/print-v8-native.pen`](fixtures/print-v8-native.pen) is a hash-pinned
minimal RGB/Lab example with exact expected preview pixels.

Validation checks channels, alpha, profile reference existence and recorded
header/channel agreement without opening external profiles. Rendering resolves
original profile bytes only for the selected page's visible paints and referenced
styles; unrelated pages, hidden nodes and disabled effects do not load their
external paint profiles. Selected profile bytes are hash/header/metadata verified
before evaluation. A batch catalog context validates once, then shares dependency
resolution across its used profiles.

Current rendering evaluates native paints with relative colorimetry into an
ephemeral sRGB8 preview. Released string-space gradients retain RGB interpolation;
explicit native gradients use their own declared space and intent. The stored native channels, alpha, profile identities,
hierarchy and extensions remain unchanged. Ordinary PNG/SVG/PDF export uses this
RGB preview; native process PDF output is still under development. Existing
compositing engine 1 behavior is retained; production linear-light precision and
native plate compositing are not yet complete.

### Developing production compositing precision

`blend::LinearSurface` provides bounded `f64` straight-alpha RGB storage for
the shared compositor. A surface carries explicit named linear RGB primaries;
blending surfaces with different primaries or dimensions fails before mutation.
The existing byte compositor and the float surface use the same source-over
and blend equations from [W3C Compositing Level 1](https://www.w3.org/TR/2024/CRD-compositing-1-20240321/#blending).
Masks and opacity multiply coverage independently of color channels. Zero
coverage canonicalizes hidden RGB to zero. Inputs are finite signed RGB
and normalized coverage; each backing allocation is bounded to 256 MiB and callers must
also account for the aggregate live memory of a render.

`value::Evaluator::composite` converts native RGB, gray, CMYK or Lab paints into
an explicit named linear RGB destination before using this same float kernel.
It returns transient `WorkingEvaluation` endpoints and their source-path clipping,
working-gamut excursion and ICC path reports alongside the composed sample.
These are distinct from stored normalized `NativeColor` values. Encoded RGB,
ICC device channels and Lab cannot be selected as this RGB blend space.
Normal source-over preserves signed and above-one RGB without clipping. The
extended-gamut policy of every other mode is fixed and tested: multiply, screen,
overlay, darken, lighten, hard-light, soft-light, difference and exclusion
evaluate their W3C formula unclamped, because each is a total function of real
channels that is continuous with its normalized form at the gamut face.
Color-dodge, color-burn, hue, saturation, color and luminosity divide by a
normalized term or clip their result into the unit gamut, so they have no
faithful extension: they refuse extended channels when both inputs contribute
and never silently clip. Any late unsupported
pixel or arithmetic overflow fails before changing the destination surface.
For the refusing modes only, values within `1e-12` of a normalized gamut face
are snapped to that face to remove matrix/blend roundoff; larger excursions fail.
Tests cover all 16 blend modes, independent partial-alpha equations, a thousand
low-coverage layers without intermediate quantization, precise masking, malformed
inputs, allocation limits, mismatched primaries and all four press intents.

`fill::render_linear` now feeds this storage through the existing fill renderer's
geometry, transforms, spread rules and four coverage subsamples. Native solid
paints and explicit linear/radial/conic gradients convert directly to signed
working RGB before premultiplied coverage accumulation. No sRGB8 paint lowering
or intermediate byte surface is involved. The caller supplies one verified
`Evaluator` context containing the used profile references; its intent must
match an explicit native gradient. Sampling reuses the prepared endpoints,
scratch native sample, compiled ICC links and named matrices without allocating
JSON, destination strings or link reports at each subsample.

Released string-space gradients retain their premultiplied sRGB/linear-sRGB
interpolation. For float output, native endpoints convert to that declared
bounded legacy domain without byte quantization; each resulting sample then
converts to working RGB before coverage accumulation. `LinearFillResult` reports
conversion clipping, including endpoint conversion into this legacy domain.
Explicit native gradients can instead preserve extended RGB in a wider declared
interpolation space. Solid paints preserve authored channel and alpha precision,
live token meaning and fractional geometry coverage.

The public helper validates the document; the shared compositor can use its
prepared counterpart after validating once for the whole scene. Both use the
same fill-local validator and sampling loop. Dimensions, scale and allocation
limits fail before pixel work, and cancellation is checked at every output row.
Rendering leaves stored documents, bindings, channels and profiles unchanged.
Pattern fills sample their sRGB8 raster asset, interpret it as sRGB and convert
each sample into the working space before coverage accumulation. The result is uncomposited fill coverage: node
opacity, masks, effects and sibling blending belong to the scene compositor.

`composite::render_working` selects float surfaces in the same ordered scene
renderer used by ordinary exports. It consumes the v8 working role and intent,
loads only visible page paint dependencies, and returns unencoded pixels plus
conversion-clipping, peak temporary-byte reservation and pixel-work reports.
The peak includes conservative reservations for pending coverage planes. Float
pixels reserve 32 bytes and coverage samples 8 bytes against the existing shared
256 MiB aggregate limit. Backgrounds, fill nodes, native vector/text fill and
stroke planes, sRGB8 image/raster/developed-photo pixels, reusable vector/raster
masks, affine placement, groups, instance fallbacks, node/content opacity and
consecutive clipping use this path. Vector and text geometry is rasterized as
coverage with the existing deterministic antialiasing contract; its original
paint is evaluated directly into working RGB and never lowered through preview
RGB8. Fill and stroke remain separate planes in their existing paint order.
Raster sources convert into the selected linear working primaries before sibling
blending, with no later RGB8 round trip. Existing byte mask resources become
normalized coverage and never quantize the working color channels. Isolated and
pass-through groups retain their existing stack semantics.

Stored `srgb` and `linear` blend spaces still mean encoded and linear sRGB,
respectively. Prepared matrices convert working primaries into the declared
blend space and back without intermediate byte encoding. Normal blending
retains extended values; unsupported non-normal excursions fail explicitly.
Blend validation checks cancellation at every row before changing a public
destination surface. Tests cover exact fractional coverage, a thousand
low-opacity layers, clipping chains, group isolation, alternate working
primaries, intent-specific CMYK paths and atomic cancellation/failure.

The float scene target also implements all thirteen released adjustment kinds.
Adjustment parameters retain their encoded sRGB meaning regardless of working
primaries: prepared matrices and signed transfer functions convert channel values
without an RGB8 intermediate. Adjustment blend formulas honor the declared
encoded/linear sRGB space, then opacity, visible sibling scope contribution and
reusable mask coverage interpolate the encoded result, matching the released
adjustment ordering without either of its byte-rounding steps. Alpha is unchanged.
Below, explicit sibling targets and group scopes share existing ordered coverage;
hidden, disabled and zero-strength operations retain skip behavior. This is RGB
appearance adjustment after native CMYK/Lab evaluation; native ink-channel edits
and plate processing remain open.

Extended adjustment channels have explicit per-operation behavior. Exposure,
brightness/contrast, invert, color balance, black and white and channel mixer
retain finite signed and above-one results. Vibrance retains extended RGB and
limits its saturation modulation weight to [0,1]. Levels maps its black/white
interval into [0,1]; curves extend their endpoint values constantly; posterize
selects normalized discrete levels; threshold selects zero or one; gradient map
extends the first/last endpoint color beyond its normalized luminance interval.
Nonidentity hue/saturation requires encoded sRGB in [0,1], snapping only 1e-12
roundoff and otherwise refusing unsupported extended HSV input. Identity
hue/saturation retains extended channels. Non-normal adjustment blends use the
same explicit extended-domain refusal as other scene blends. Any conversion or
channel arithmetic overflow fails. A bounded scratch surface commits only after
every affected pixel and row cancellation checkpoint succeeds; its allocation and
scope work count against aggregate renderer budgets. Focused adjustment tests
cover every operation, scopes, masks, alternate primaries, blend spaces, precise
alpha, extended endpoints and atomic late failure/cancellation.

Ordered color and gradient overlay effects now also use working float paints.
Native solid colors and linear/radial/conic gradients reuse the prepared fill
evaluator, selected native gradient intent and explicit working primaries.
The original masked source silhouette remains immutable across the stack:
content opacity applies first, overlays use source coverage, and final node
opacity applies afterward. Blend formulas honor their declared coordinate space;
overlay strength interpolates encoded sRGB as in the released effect engine,
without quantizing color or alpha. Normal blends preserve extended channels and
other modes retain the shared explicit domain policy. A whole ordered stack
commits only after all rows succeed, including cancellation checkpoints. The
renderer accounts for its result and one simultaneous paint surface, plus fill
and overlay work, against aggregate limits. Disabled effects keep their existing
skip path. Tests cover native overlay pixels, stack order, fractional alpha,
content/final opacity, gradient geometry and atomic late failure/cancellation.

Spatial effects (blur, drop/inner shadow, outer/inner glow and stroke) also run
on working float surfaces. They mirror the released effect engine's geometry:
the three-box blur approximation on premultiplied encoded sRGB, offsets rounded
after render scale, the blurred (or complemented, for inner kinds) source alpha
as effect strength, and the octagonal distance field for stroke positions. Effect
colors resolve through the native color evaluator into working primaries, and
color, coverage and alpha are never quantized. Inner kinds mix inside the source
silhouette, drop shadows and outer glows composite beneath the content, and
strokes composite above it, each with the declared blend mode and space. A scaled
blur radius above 256 pixels fails as a limit before work begins. The renderer
reserves three extra full float surfaces for a stack with any spatial operation
and charges its work against the aggregate pixel budget. Tests compare every
kind, stroke position, normal and multiply blends, and render scales 1 and 2
against the released byte engine, assert exact fractional shadow and stroke
alpha, and cancel at every checkpoint without mutating the source.

Transform stacks that cannot fold into one direct affine (perspective or
four-corner homographies, weighted opacity or masked operations) also resample
working float surfaces. Byte and float paths share one inverse mapper, so they
sample identical source positions, and one premultiplied bilinear kernel over
encoded sRGB coordinates, as in the released resampler. The float path never
rounds color or alpha and retains extended working channels. It holds the warped
result and one encoded copy of the source against aggregate limits, checks
cancellation on every input and output row, and replaces the source only after
the whole surface succeeds. Tests compare perspective, weighted and partially
disabled stacks at scales 1 and 2 against the byte renderer, assert exact
fractional alpha for a weighted half-pixel shift, and cancel at every checkpoint.

Pattern fills interpret their sRGB8 raster as sRGB. Hidden content
and disabled operations keep their existing skip semantics. Ordinary exports
continue to use the documented sRGB8 preview. `color::output` is the explicit
delivery boundary for float renders:

`output::deliver` (or `output::render`, which renders a page with
`composite::render_working` first) converts each unquantized linear working
pixel through D50 PCS to an encoded destination and rounds half up with
saturation exactly once, to 8 or 16 bits. The destination and intent default to
the document `output` role, which must be configured (`color role assign`) or
the call fails naming the role; both may be overridden explicitly. Supported
destinations are named encoded RGB and ICC RGB, gray and CMYK profiles whose
requested intent path exists (a profile without a B2A path cannot be an output
destination). Lab has no unambiguous integer encoding and linear RGB is not a
device encoding, so both are refused. Alpha is either preserved as a final
straight channel (RGB and gray only; CMYK refuses non-opaque pixels) or
flattened over paper white in linear working light. The result reports pixel,
clipped-pixel and quantized-sample counts, the largest normalized rounding
error and the profile identity, and never mutates the document. Cancellation is
checked on every row; the sample budget is the float-surface byte limit.
Production exports (PDF/X, plates, proofs) remain open and must build on it.

Traversal permits at most 65,536 native slots, 64 levels of scene/paint reference
nesting, and 4,096 UTF-8 bytes per native color JSON pointer, including resolved
fallbacks. Oversized traversals fail before profile loading or output writing.
Diagnostic owner labels are bounded to 128 characters. Malformed native edits
fail the shared transaction validation without writing or creating history.

### Implemented paint assignment and conversion

`color paints assign DOCUMENT --to DESTINATION` changes paint interpretation
while retaining every channel number and alpha; channel counts must match and
the assigned values must fit the destination's declared range. `color paints
convert DOCUMENT --to DESTINATION --intent INTENT` evaluates the selected
transform, retaining alpha separately and reporting clipping. `DESTINATION` is
the same bounded JSON object used by `color evaluate --to`. The default intent
is relative colorimetric. Assignment checks the destination interpretation is
executable; it can replace a missing external source profile without reading
that source's original bytes. Conversion verifies both required dependencies.

These edits cover supported stored paints, including canvas backgrounds, hidden
nodes, disabled effect colors, fallbacks and referenced styles and vector masks.
Default scope is the first page, global `--page ID` selects a page, and
`--all-pages` also includes unused shared styles, component roots and mask
snapshots. Matched locked paints reject the whole plan. Changing a shared style,
component root or vector mask requires `--include-shared`; locked users on other
pages also reject the change, including through style alias chains and masks.
Immutable mask payloads receive new content identities, with affected aliases
and attachments replaced together. A replacement removes the superseded table
entry; the transaction history retains the original document. No modified bytes
are stored under the old digest. Snapshot geometry and extension data survive.

`--legacy-srgb` is an explicit conversion target, mutually exclusive with `--to`.
It quantizes to sRGB8 `#rrggbb` or `#rrggbbaa`, reports clipped and quantized sample
counts, and permits downgrade when no native declarations or ICC catalog entries
remain. It does not remove retained profiles. Named native sRGB remains floating
point when selected through `--to`.

Planning validates locks, shared-resource consent, profile execution and final
scene/size limits before writing. Commit uses the exact original-byte revision
even without an explicit `--if-revision`. Dry run and commit use the same planner;
an edit creates one recoverable history transaction. Offset and limit (1–256)
paginate changed paint records without restricting the planned mutation.
`matched` counts selected paint samples; `matches`/`changed` count changed values;
`returned`/`offset`/`limit`/`has_more` describe the emitted `changes` exactly.
Resource replacements are separately bounded by the existing 256 mask limit.
No matching samples is an error. These commands do not change raster samples or
working/proof/display/output roles, and do not claim production output support.

### Remaining native scene contract

Color values declare `space`, `channels`, optional `profile` and separate `alpha`.
Spaces are named RGB, ICC RGB, ICC gray, ICC CMYK, D50 Lab or spot. Spot values
reference a stable ink ID and tint [0,1]; their alternate color is defined by the
ink record. All color-bearing slots must participate: fills, strokes, named
styles and fallbacks, gradient stops, canvas backgrounds, text, image/raster
interpretation, effect colors, masks where color is meaningful, components and
instance overrides. Diff, replace, import and package operations preserve and
inspect these declarations. Conversion recursively traverses the selected page
and referenced resources with explicit handling of shared styles/components.
It must never change another page through a shared resource without consent.

Working compositing uses an explicitly declared linear RGB space; conversion of
CMYK/Lab/spot alternates to preview RGB does not overwrite native channels.
Native process and spot plate evaluation is a separate output of the same scene
evaluation, retaining coverage and ink identity. Unsupported transparency/ink
interactions fail production preflight. Gradients declare an interpolation space
and interpolate alpha separately; interpolation in device CMYK preserves channel
values while appearance conversion uses the selected ICC profile. Photography
stays scene-linear ProPhoto internally and converts once into scene/output space.

Migration preserves IDs, hierarchy, stacking, locks, unknown extension fields,
photo settings and source hashes. It makes legacy sRGB interpretation explicit
without numerical conversion. Reverse migration refuses native CMYK/Lab/spot,
overprint and production semantics that the target version cannot represent.

## Proofing, inks and plate behavior

Proofing follows source -> print device -> print PCS -> display. Paper simulation
uses absolute colorimetry; ink-black simulation uses the profile's reproduced
black. These are independent explicit controls, reported with the transform path.
Preserve-numbers bypasses reseparation only when source and output process channel
spaces and profile identity match. Gamut warnings compare the source PCS to the
print round trip with a recipe threshold in CIE76 Delta E. Display assumptions,
browser color handling and lack of instrument calibration are shown in proof
setup. A proof PNG embeds its destination RGB profile. Browser panels request
Rust proofs and never implement an independent color transform.

Ink analysis reports per-channel minima/maxima, TAC=sum(C,M,Y,K)*100, above-limit
coverage, rich black, registration black and small text using multiple plates.
TAC thresholds and small-text point sizes belong to the recipe and are never
guessed from a filename. Preserve-black and neutral policies must name a tested
algorithm; generic `1-R` CMYK formulas are not production separations.

Offline ink records contain stable ID, canonical name, aliases, tint alternate,
provenance and redistribution policy. Ink identity survives packaging. Duplicate
aliases and conflicting definitions require an explicit resolution. Unlicensed
commercial ink libraries are not bundled. A process-only preview reports spots
that it displays through alternates.

Knockout clears underlying plate coverage in the painted area before applying
the object. Overprint retains eligible underlying channels; fill and stroke have
separate settings and a specified overprint mode. Zero process channels follow
that mode rather than always knocking out. Spot plates remain separate from
CMYK and from each other. Unsupported blend modes, partial-alpha spot interactions
or overprint in isolated transparency groups fail preflight until their plate
semantics have reference coverage. Plate exports use deterministic channel order
C,M,Y,K followed by spot IDs in lexical order and carry ink maps and dimensions.

### Plate separations (implemented subset)

`pentool color separate <doc> --out <dir> [--page ID] [--to JSON --intent I] [--scale S]`
evaluates the selected page into ink plates and writes one 8-bit grayscale PNG
per plate (`plate-NN-C.png` ... `plate-NN-spot.png`; white is no ink, black is
full ink) plus `separations.json` carrying dimensions, ink map, per-plate
statistics and PNG SHA-256 digests. Existing files are never replaced. The
document is not modified.

Image, photo, raster, and scene3d nodes contribute flattened process-color
coverage. Scene3d count is reported as `flattened_scene3d_objects`, with a
`warnings` entry that linked spot inks, vector overprint, and source-texture
plate semantics are not preserved. Raster nodes with overprint, trap, or
style-level spot declarations are rejected rather than silently treating those
vector-only declarations as if they affected the flattened pixels.

Declarations are ordinary `style` fields, so preview rendering is unchanged:

- a paint with `"spot": {"ink": ID, "tint": 0..1}` deposits only that ink; its
  mandatory `fallback` native color is the preview alternate and must be opaque;
- `"overprint": {"fill": bool, "stroke": bool, "mode": 0|1}` selects overprint
  per slot; absent or `false` knocks out;
- process paints convert to the CMYK `output` role (or `--to`) through the ICC
  engine; an `icc-cmyk` paint using the same profile preserves its numbers.

Knockout replaces every plate under the painted coverage. Overprint retains
plates the paint does not replace: a spot overprint touches only its own plate;
a process overprint replaces C,M,Y,K in mode 0 and only non-zero channels in
mode 1; spots are always retained. Antialiased edges interpolate each affected
plate by 8-bit coverage. Plate order is C,M,Y,K then used spot IDs in lexical
order; process plates are always present and blank when unused, and a CMYK
destination is required only when a process paint exists.

Separation fails, naming the node, for opacity below 1, non-normal blend modes,
effects, masks, clipping, transforms, partial alpha (including spot), gradients,
patterns and other non-vector node kinds (plain images and gradient fills
separate, see below), overprint inside isolated
groups, and more than 4096 painted slots or 36 plates. Trapping, photo-catalog
separations and a composite plate preview remain open.

### PDF/X-4 export (implemented subset)

`pentool color pdfx campaign.pen out.pdf [--to JSON --intent NAME] [--ppi 300]
[--title T] [--created 2026-10-10T12:30:00Z]` writes a **flattened CMYK** PDF 1.6
draft: the selected page is delivered once to the CMYK output profile (alpha
flattened over paper white) and stored as one ICCBased image cropped to the media
box. The page must declare `production` geometry; `MediaBox`, `BleedBox` and
`TrimBox` come from it in points (the art box is refused for PDF/X). The profile
is embedded as the `GTS_PDFX` output intent and must be an ICC v4-or-older CMYK
`prtr` profile. Info and XMP carry `PDF/X-4`, `Trapped False`, title and dates.

The output is deterministic: no wall clock (`--created`, else `SOURCE_DATE_EPOCH`,
else the Unix epoch), content-derived trailer `/ID`, fixed Flate level. Spot
paints and overprint declarations, a missing output role or geometry, oversized
rasters and existing output paths fail before anything is written; the write is
atomic.

Limits, stated plainly: text and vectors are rasterized (no font programs, no
live transparency), and spot inks are not carried. The report records
`independent_validation.status = "not-run"`: no PDF/X conformance is claimed until
a named validator's name, version, ruleset and result are recorded. The built-in
structural check only proves xref/trailer consistency.

### Preflight (implemented subset)

`pentool color preflight campaign.pen [--min-ppi 150] [--bleed-scale 1]` checks
the selected page and prints `{ok, errors, warnings, issues[]}`. Each issue has a
stable `code`, `severity` (`error` blocks production, `warning` needs review),
`node`, `message` and `fix`. It is read only (`mutated: false`) and caps output at
500 issues, reporting `issues_omitted`. The report also lists up to 500 `overprint_nodes` (`id`, `kind`, and top-level `x`, `y`, `width`, `height`) that the print view highlights.

Checks: output role present, ICC CMYK, catalog profile loadable, `prtr` class and
v4-or-older (`output-role-missing`, `output-not-cmyk`, `output-profile-*`);
production geometry valid and present with PDF/X box rules (`production-*`);
bleed under 1/8 inch and unpainted bleed bands measured from rendered alpha
(`bleed-below-minimum`, `bleed-incomplete`); nominal image resolution from the asset
pixel size and node width (`image-low-resolution`, `image-unresolved`); scene3d
output raster dimensions, page units, and accumulated final affine placement
(`scene3d-low-resolution`, reported separately in `scene3d_ppi`); each scene3d
mockup also emits `scene3d-print-flattened`, because linked spot inks, overprint,
traps, and source vector plates are not retained by rasterization. Preflight
follows visible linked page/node artwork and packaging-panel sources for spot
paints (`scene3d-spot-flattened`); the recipe's `reject_spot_flattening` policy
turns that finding into an error. This audit is capped at 100,000 visited nodes
and reports `scene3d-spot-audit-incomplete` if it cannot finish; text,
live transparency, spot and overprint constructs the flattened PDF/X subset
cannot carry (`text-rasterized-in-pdfx`, `transparency-flattened`,
`spot-not-in-pdfx`, `overprint-not-in-pdfx`).

Not yet checked: total ink coverage, font licensing/embedding, missing-glyph
fallback, image color-space mismatches, rich-black policy and recipe thresholds.

### Print recipes (implemented subset)

A recipe is a small JSON file (at most 16 KiB) passed with `--recipe` to
`pentool color pdfx` and `pentool color preflight`:

```json
{"version":1,"name":"magazine-print","pdf_standard":"PDF/X-4",
 "intent":"relative-colorimetric","ppi":300,"min_ppi":200,
 "min_bleed_inches":0.125,"bleed_scale":1}
```

Only `version` and `name` are required. Unknown fields, out-of-range numbers and
any `pdf_standard` other than `PDF/X-4` are rejected so a typo never silently
drops a threshold. Explicit flags always override the recipe. Recipes supply
rendering intent, export resolution, minimum image resolution, minimum bleed and
the bleed-measurement scale, total ink limit, and an optional `profile` (a
catalog name of an ICC CMYK output profile; it needs an `intent` and is used as
the destination when `--to` is not given), `plate_bits`, `auto_trap` and `scale`
for plates (see "Recipe plate depth"), and `allowed_spots`: up to 32 ink IDs
(1 to 64 characters of A-Z a-z 0-9 - _ ., no repeats). With `allowed_spots`
set, preflight (and so `color package`) reports a `spot-ink-not-allowed` error
naming each node whose fill or stroke uses any other spot ink; without it every
ink is allowed. `black_policy` is `"any"` (default) or `"k-only-text"`; the
latter makes preflight report a `text-not-k-only` error for each text node whose
ICC-CMYK fill carries cyan, magenta or yellow. `require_marks: true` makes preflight report `marks-missing` when the page
declares no printer's marks. `compression` is `"flate"` (default) or `"none"` and sets whether `color pdfx`
and `color package` Flate-compress the page image and profile streams; the PDF
report records it. `spot_policy` is `"convert"` (default; spot paints the flattened PDF/X export
cannot carry are a `spot-not-in-pdfx` warning) or `"reject"` (the same finding
is an error, so `color package` refuses the job). `spot_substitutions` is an object of at most 16
`"from-ink": "to-ink"` entries (ink IDs, no self-maps or chains): before separation, preflight or
PDF/X export the CLI moves every spot paint of the first ink onto the second ink, keeping its
tint and fallback. Both inks must be in the document's library (else an error names the ink); the
change is in memory only, and `color package` still copies the original `document.pen`.
Spot-to-process conversion is not implemented. Resampling is not a separate
recipe field: the draft PDF/X export flattens the page to one CMYK raster, so
every embedded image is resampled once at the recipe `ppi`. Per-image
resampling arrives only with vector/live-image PDF output.

A document may embed up to 16 recipes in a top-level `print_recipes` array (each
a full version-1 recipe with a unique `name`). `--recipe @NAME` on `color
separate`, `preflight`, `package` and `pdfx` reads the named one instead of a
file; a missing name is a `[missing-resource]` error and the document is not
modified (tests/v0120_recipe.rs).

### Production packages (implemented subset)

`pentool color package campaign.pen --out pkg [--recipe r.json] [--to JSON --intent N]`
builds a directory containing `document.pen` (the exact source bytes),
`output.pdf` (the unvalidated PDF/X-4 draft), `preflight.json`, `plates/*.png`
with `plates/separations.json`, `profiles/<sha256>.icc` for every profile a color
role depends on (verified by the catalog loader), and `package.json`, a manifest
of every file's role, size and SHA-256 plus the PDF/X report and
`independent_validation`.

The package is staged beside the target and renamed into place. It refuses when
preflight finds errors (the codes are listed) and never replaces an existing
path. Plates are best effort: when the page uses constructs outside the
separation subset the manifest records `plates.status = "skipped"` with the
reason instead of failing the package. `pentool color package-verify pkg`
re-hashes every listed file offline and rejects manifest paths that are absolute,
contain `..`, `\` or `:`.

`proof.png` is a soft-proof image: the delivered output CMYK read back through the
output profile to PCS and on to the display role, bounded to 1,000,000 pixels. It
is a numeric profile simulation with no paper or spectral model; when the roles
cannot support one the manifest records `proof.status = "skipped"` with the reason.
`ink-map.json` lists per-plate average and maximum coverage and the maximum total
process ink in percent.

Linked images (external storage) are re-verified against their digests and copied
at their document-relative paths, so the packaged `document.pen` resolves them
unchanged; they appear in the manifest with role `image-asset` and in the
`assets` summary. Embedded images stay inside the document.

Not yet included: font collection (`assets.fonts` is `not-collected`), per-recipe
contents, and compression or signing of the package.

## Production geometry and delivery

Page production geometry declares trim, bleed, media, optional art box, slug
and safe-zone in document units with explicit document-units-per-inch. PDF boxes
convert to points using 72 points/inch, including the top-left to bottom-left
coordinate conversion. Required containment is media >= bleed >= trim; art box
and trim box cannot both be used to express the same PDF/X page boundary.
Marks live outside trim, fit media and identify their registration plate behavior.
Bleed detection measures actual painted coverage, not just object bounds.

Implemented subset: a v8 page may carry `production` with `units_per_inch`
(default 72, at most 2400), `trim` (default the canvas), `bleed` and `slug`
(a number or top/right/bottom/left object), optional `art`, `safe` insets and
`marks` (`crop`, `registration`, `offset`, `length`). The media box is the bleed
box grown by the slug and must fit the canvas; marks must start outside the
bleed and fit the slug; the art box must lie in the bleed box and is refused
for PDF/X together with a trim box. `pentool color production show` reports
the boxes, PDF boxes in points relative to the media origin (bottom-left,
72 pt/inch), mark geometry, and with `--coverage-scale` the measured painted
alpha of each bleed band. `set` and `clear` use the shared transaction path.

Manual trapping uses explicit editable spread/choke widths and ink pairs.
Automatic trapping initially supports only opaque, flat, adjacent vector fills
without effects, text or images; unsupported candidates produce actionable
issues. The algorithm, ink order, width and joins must be recorded, deterministic
and covered by plate fixtures before the control is exposed.

PDF/X-4 export embeds the verified output ICC profile, required metadata, valid
page boxes and every used font (or explicitly outlines text). It preserves
supported transparency in a declared blending space. Unsupported PDF constructs
fail before final output is written. The generic PDF writer remains a generic
export path until this separate production mode is implemented. External links,
encryption, executable content and missing resources fail production delivery.

Independent validation records validator name, version, ruleset, exact output
SHA-256, result and report hash. A failed validator leaves no final production
artifact; draft generation is explicitly labeled unvalidated. The validator is
a development/delivery check, not an undocumented required native runtime of
Pentool. Engine-only tests cannot substitute for independent validation evidence.

## Preflight, recipes and collection

Preflight produces deterministic issues with stable code, severity, page, object,
resource, measured value, threshold and corrective action. It covers profiles,
licensing, unsupported transform paths, TAC, spots, overprint/transparency, fonts,
effective image resolution after placement, bleed coverage, page boxes, metadata
and the supported PDF/X rules. Filters and pagination describe exactly the
emitted issues. Export executes the same preflight planner and blocks on errors.
Warnings require the recipe's explicit policy. Invalid IDs never become an empty
successful report. Page selection limits analysis and export to that page unless
all pages are requested.

Recipes record output/proof profile hashes, intent, black policy, spot policy,
resampling, compression, geometry, marks, PDF standard, thresholds and warning
policy. Recipe versions are explicit and unknown future versions fail. There is
no universal built-in magazine press profile: a recipe needs a verified printing
condition. Production packages collect the editable master, verified dependencies,
recipe, proofs, plates, ink map, preflight and validation reports and final output
with a manifest of hashes. Existing package verification, path containment and
atomic write/recovery mechanisms must be extended. Reproduction offline must
produce identical output bytes on every supported target.

## Evidence gates

The fixture family is `docs/fixtures/print-*`; pinned source bytes have LF/binary
attributes as appropriate. Tiny synthetic profiles are authored for Pentool and
redistributable; they are computational probes, not press characterizations.
Reference vectors identify provenance, intent, encoding and numeric tolerance.
At least one independently evaluated RGB and one real, redistributable CMYK
profile must supplement synthetic probes before engine completion.

The initial RGB and CMYK probes can be regenerated with
`python scripts/generate-print-fixtures.py`. Their exactly representable
RGB8/CMYK8 endpoint/primary vectors can be independently evaluated with
`python scripts/check-print-probes.py` (development-only Pillow/LittleCMS).
The latter writes `target/print-probe-lcms.json` with CMM version, profile hashes
and measured CIE76 differences. Its tolerance is 1.5 Delta E to allow LAB8
quantization; Rust tests compare the unquantized XYZ vectors to 1e-12. Neither
check certifies a press condition or substitutes for LUT/CMYK/plate evidence.

`python scripts/check-print-schema.py` uses development-only Python `jsonschema`
to validate the hash-pinned v8 documents and positive/negative native sample
and destination grammar cases. It registers reference schemas locally and refuses retrieval,
then records the validator version and exact fixture hashes in
`target/print-schema.json`. This checks JSON grammar; Rust's byte/profile checks
remain necessary, and neither check certifies production output.

`python -I scripts/check-pdfx.py FILE.pdf` is a standard-library-only structural
check of a `color pdfx` draft. It shares no code with Pentool and verifies the
header, xref offsets, trailer IDs, output intent, CMYK ICC stream, Info/XMP
PDF/X version agreement, MediaBox/BleedBox/TrimBox nesting, stream lengths and
image sample counts, and rejects encryption, scripts and device RGB/gray. Its
JSON states `conformance_claim: false`; it is not a PDF/X validator, and the
export report keeps `independent_validation.status` at `not-run`.

Required evidence includes v2/v4 XYZ/Lab endpoints; every curve and LUT form;
intent-specific paths; absolute white scaling; gray/RGB/CMYK/device-link vectors;
spot tint and overprint plates; transformed and nested scene nodes; mixed pages;
locked/shared resources; failed-edit byte preservation; malformed/truncated/
overlapping/oversized profiles; hostile PDFs; missing fonts/profiles; cancellation;
offline collection reproduction; and independent PDF/X validation. Every visual
change needs semantic and pixel/plate assertions, with explained golden changes.

All CI targets must pass formatting, warnings-denied Clippy and locked tests.
Benchmark profile compilation, RGB/CMYK transforms, proofs, plates and preflight
with bounded memory on the same release build and machine. Record toolchain,
profile hashes, document size, pixels, timing and peak memory without claiming
cross-platform performance from a local run. Roadmap checkboxes stay open until
their implementation, documentation and required evidence exist.

## Production marks in output (implemented subset)

Declared `production.marks` (crop, registration) are painted into outputs, not
only reported. Marks are 0.25 pt hairlines (never thinner than one device pixel)
in registration color: 100% of every process ink. They appear in the PDF/X
raster (`marks.rendered`, `marks.ink = "registration"` in the report) and on the
C, M, Y and K plates (`marks` in the separation report); spot plates are left
untouched. Geometry is the same `production::marks()` data the preflight uses, so
what is reported is what prints. Color bars and slug text are described below;
automatic trapping is not implemented.

## Manual trapping (implemented subset)

`style.trap: {"width": <units>}` on a vector or text node spreads that node's
ink outward by `width` document units using a square structuring element
(bounded to 32 device pixels at the separation scale). In the fringe, each plate
takes the larger of its existing value and the node's value, so the trap only
ever adds ink under neighboring paint; objects painted later still knock the
fringe out. `trap` is refused on nodes that also declare `overprint`, and
malformed declarations fail document validation. The count is reported as
`traps` in the separation report. Automatic trapping, choke direction and
per-edge rules are not implemented and the PDF/X raster path does not trap.

## Total ink limit (implemented subset)

`pentool color preflight --max-total-ink <percent>` (or recipe `max_total_ink`,
100..=400) renders the page through the output role to 8-bit CMYK (bounded to
about four million pixels) and reports `ink-total-exceeded` as an error when any
pixel's C+M+Y+K exceeds the limit, with the pixel count and worst value. The
measurement is on the profile's delivered values, so it reflects the press
conversion and any black policy in effect. It is a sampled raster check, not a
vector paint audit; with no limit configured nothing is measured.

## Gradient separations (implemented subset)

A `fill` node whose `fill` is a `linear`, `radial` or `conic` gradient with
opaque native stops separates onto the process plates. The gradient geometry,
spread and transform are rendered once as an 8-bit position ramp; each stop is
converted to the CMYK destination and the four device values are interpolated
linearly between stops at that position. A K-only ramp therefore stays K-only.
Position resolution is 1/255. Refused with `[unsupported-capability]`: stops with
partial alpha, a declared native interpolation `space`, and non-gradient fill
kinds. Gradients carry no overprint or trap; they knock out beneath them.

## Image separations (implemented subset)

An `image` node separates by rendering the node (crop, fit and its own
operations) to straight-alpha sRGB8 and converting each distinct pixel color
through the output profile to C, M, Y, K. Results are cached per color, so the
conversion is deterministic. Images knock out beneath them and carry no overprint
or trap; opacity, blend, effects and masks are refused as for vector nodes.
A `photo` node (a placed catalog variant) takes the same path: it is developed
and rendered like any export, then separated, so its output is limited to
8-bit sRGB (see tests/v0120_separation.rs). Spot-ink images are not separated;
wider-gamut photo development is converted to sRGB before the profile step.

## Automatic trapping (implemented subset)

`pentool color separate --auto-trap WIDTH` (or a recipe's `auto_trap`; the flag
wins) post-processes the finished plates. Where a lighter area meets a darker one
- perceptual ink density (C 0.30, M 0.59, Y 0.11, K 1.0, spots 0.5, capped at 1)
differing by at least 0.25 - the lighter neighbor's ink is spread up to `WIDTH`
units under the darker area, taking the larger value per plate. The widest spread
is 8 device pixels (`[limit-exceeded]` beyond that at the chosen `--scale`).
Evaluation reads a snapshot, so it is order independent and deterministic, and it
runs before marks, slug text and color bars so those stay untrapped. The report
carries `auto_trapped_pixels`. Not implemented: per-object trap rules, trap
direction hints, centerline traps, trapping of rich-black neutral density by
color-specific rules, and any claim of conformance with a commercial trap engine.

## Composite plate preview (implemented subset)

`pentool color separate --composite` also writes `composite-preview.png` into the
plate directory (never replacing a file) and reports it under
`composite_preview` with its SHA-256. Each pixel's C, M, Y, K values (quantized
to 8 bits) are read through the separation's ICC output profile to PCS and on to
the document display role, as `color proof-image` does. Every spot plate is then
laid over the result as a multiply of its ink's sRGB alternate at the plate's tint.
The spot step is an approximation: there is no spectral or paper model, and
spot inks whose alternate is not `srgb` are refused with
`[unsupported-capability]`. Previews above 4,000,000 pixels are refused with
`[limit-exceeded]`; separate at a lower `--scale`. The preview is not a contract
proof and is not stored in the plate manifest.

## Recipe plate depth (implemented subset)

A print recipe may carry `plate_bits` (8 or 16). `pentool color separate
--recipe FILE` takes plate bit depth, intent and the `profile` destination from
it; `--bits`, `--to` and `--intent` override the recipe. Any other value of
`plate_bits` is refused with `[invalid-input]`. `pentool color package --recipe`
applies the recipe's `plate_bits`, `auto_trap` and `scale` (plate render scale,
0.1 to 8; `--scale` overrides it on `color separate`) to the packaged plates as well.

## Color bars (implemented subset)

`production.marks.color_bars: true` adds eight patches of `length/2` by `length`
units along the bottom slug, starting at the trim's left edge and `offset` below
the trim: C, M, Y, K, R (M+Y), G (C+Y), B (C+M) and 50% K. Patches knock out
whatever lies beneath and carry only those process inks; spot plates are cleared.
They print into the PDF/X raster (`marks.color_bar_patches`) and the C, M, Y, K
plates (`color_bar_patches` in the separation report). The page must have a bar
of at least `9 x length` units of trim width so the bar clears the center
registration mark.

## Slug text (implemented subset)

`production.slug_text` is a string of 1..=200 printable ASCII characters printed
at 6 pt in registration color (100% of every process ink; spot plates stay
clear) in the top slug, starting one mark length right of the trim's left edge
(or at the trim edge when no marks are declared) with its baseline about one
line below the media top. It is rendered by the scene's own text pipeline with
the built-in Atkinson Hyperlegible face, so it prints identically everywhere.
The page is refused when the top slug above the marks is shorter than 1.4 times
the text size or when the estimated line width exceeds the trim width less the
mark margins. It appears in the PDF/X raster (`marks.slug_text`) and on the C,
M, Y and K plates (`slug_text` in the separation report). Variables such as
page numbers, dates and fonts other than the built-in face are not implemented.
