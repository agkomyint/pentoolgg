# Photography and RAW development (v0.11, photo engine 1)

Status: **specification frozen for review** (roadmap v0.11.0 item 1). Nothing in this
file is implemented yet. Later items implement it in order and must not contradict
it. A deliberate change edits this file and the schema in the same commit.

This contract adds a non-destructive photography workflow to the v6 scene graph:
immutable photo sources, a catalog of photos with editable development settings,
variants and snapshots, local masks, merges, organization, metadata policy, and
output recipes. Schema: [`pen-format-v7.schema.json`](pen-format-v7.schema.json).
Conformance fixtures: [`fixtures/photo-conformance.json`](fixtures/photo-conformance.json).

## Principles

1. **Sources are immutable.** RAW and imported photographs are content-addressed by
   `sha256:` over their exact bytes. No command rewrites them. Every other piece of
   data (settings, masks, crops, variants, recipes) is editable JSON or a
   content-addressed resource.
2. **The document records every decision.** Rendering never depends on an implicit
   default other than identity, a camera database, a network service or an AI
   provider. Import writes its suggested starting values (for example RAW
   sharpening) explicitly into the master variant.
3. **Analysis runs once.** Automatic white balance, auto tone, auto upright, sampled
   neutrals and AI-derived masks run when a command is issued. Their results are
   stored as explicit values, together with `auto` provenance
   (`{"algorithm": "...", "version": N}`). Rendering never runs analysis again.
4. **Processing is deterministic.** For the same document and build, every platform
   produces byte-identical 16-bit output (see [Determinism](#determinism)).
5. **Work is bounded.** Every limit below is checked before decoding or
   allocation. Long-running work (decode, merge, export) can be cancelled, and a
   cancelled or failed command leaves no mutation and no orphan files.
6. **Existing appearance is preserved.** Existing v4–v6 documents, image nodes,
   raster layers and the `srgb8` page compositor are untouched. Photographs enter
   the page through a new `photo` node that converts to the page's compositing
   space at its own boundary.
7. **Privacy is explicit.** Exports strip metadata unless a recipe opts in by
   category. GPS, serial numbers, and owner identity are never copied into
   document JSON.

## Document version 7

Photography requires `"version": 7`. A v7 document is a v6 document (compositing,
masks, raster layers and image nodes are unchanged) plus:

- an optional top-level `photography` object (the catalog, below);
- a new page node kind, `photo`.

Version policy:

- A pentool build that predates v7 refuses a v7 document with
  `[unsupported-capability] document format version 7 is newer than supported
  version 6` and does not modify it. This is why photography uses a new version
  instead of adding a node kind to v6: the failure is clear rather than an
  "unsupported node kind" deep in validation.
- `pentool migrate doc.pen --target 7` upgrades v1–v6 losslessly. It runs the v6
  migration and then sets `version: 7`. It does not add a `photography` object. The
  first photo command upgrades a document implicitly, in the same transaction and
  history entry, just as `image add` upgrades to v5 and `raster add` to v6.
- `--target 6` succeeds only when the document has no `photography` object (or an empty
  one) and no `photo` node. Otherwise it fails with `[unsupported-capability]
  version 6 cannot represent photo <id>; export it as an image first`.
- Document-level `compositing.color_space` stays `srgb8`. Page appearance does not
  change when a document is upgraded.

## Sources

`photography.assets` maps a `sha256:` digest to an asset record. It is separate
from `image_assets`, because v5/v6 code assumes image assets are 8-bit PNG/JPEG/WebP.
The same bytes may appear in both maps. Storage uses the shared resource contract
(`embedded` base64 or `external` document-relative path, the same path-safety rules,
`hash-mismatch` and `missing-resource` codes).

| `media_type` | `kind` | Accepted input |
|---|---|---|
| `image/x-adobe-dng` | `raw` | DNG subset below |
| `image/tiff` | `rendered` | baseline RGB TIFF, 8 or 16 bit, uncompressed or deflate |
| `image/png` | `rendered` | 8- or 16-bit RGB/RGBA PNG |
| `image/jpeg`, `image/webp` | `rendered` | as for v5 image assets |
| `image/x-adobe-dng` | `derived` | a pentool merge output (a DNG pentool wrote) |

Record fields: `media_type`, `kind`, `byte_length`, `storage`, `pixel_width` and
`pixel_height` (after the default crop, before orientation), `orientation` (1–8),
`bit_depth`, `input_profile`, `capture` (searchable summary, below) and, only
for raw and derived assets, `raw`. Derived assets also carry `derived`. Every field
other than `storage` is a fact read from the bytes. A reader that decodes the source
verifies these facts and reports a mismatch as `[malformed-resource]`.

Size: a source may be up to 512 MiB, but only sources up to 128 MiB
(`MAX_SOURCE_BYTES`) can be embedded. A larger one must use `--external`; an
`--embed` request is refused with that corrective action. Embedded photo and image
bytes together count toward the existing 512 MiB document limit.

### Input profiles

`input_profile` declares how source pixel values map into color:

- `"camera"`: raw and derived sources. The camera profile (below) maps them.
- A named RGB space from the [color space table](#rgb-color-spaces) (`"srgb"`,
  `"display-p3"`, `"adobe-rgb-1998"`, `"prophoto"`, `"rec2020"`).
- `{"icc": "sha256:..."}`: an embedded ICC profile, copied into
  `photography.profiles` as a content-addressed resource. In v0.11 only
  matrix/TRC profiles (ICC v2 or v4 with `rXYZ`/`gXYZ`/`bXYZ`, `wtpt` and `curv` or
  `para` TRCs, at most 4096 curve points) are accepted. A LUT-based profile is
  refused with `[unsupported-capability]`, and the corrective action is to re-import
  with `--input-profile NAME`. Full ICC support is v0.12.0.

A rendered source with no embedded profile is `"srgb"`, which is what v5 image
assets already assume. `--input-profile NAME` overrides the declaration explicitly,
and the override is recorded.

## RAW support (DNG subset, own decoder)

RAW decoding is pentool's own bounded TIFF/DNG reader. It adds no dependency.
Deflate uses the `flate2` crate already present in `Cargo.lock`. Lossless JPEG is
implemented in-tree.

**Containers.** A file is a DNG when it is a classic TIFF (`II*\0` or `MM\0*`, not
BigTIFF) and IFD0 holds `DNGVersion` (50706) between 1.0.0.0 and 1.7.1.0, with
`DNGBackwardVersion` at most 1.7.1.0. Other RAW containers are recognized by
signature: CR2 (`II*\0` plus `CR` at offset 8), CR3 (ISO-BMFF `crx `), NEF, NRW, ARW,
SR2, ORF (`IIRO`, `IIRS`), RW2 (`IIU\0`), PEF, RAF (`FUJIFILMCCD-RAW`), 3FR, IIQ and
X3F. They are refused with `[unsupported-capability] <container> raw files are not
supported; convert to DNG first (for example with Adobe DNG Converter)`. A TIFF
without `DNGVersion` is handled as a rendered TIFF.

**Raw image.** The main image is the single IFD with `NewSubfileType = 0`, found in
IFD0 or its SubIFDs. Supported layouts:

| Property | Supported in engine 1 | Otherwise |
|---|---|---|
| `PhotometricInterpretation` | 32803 CFA; 34892 LinearRaw with 1 (monochrome) or 3 samples | `unsupported-capability` |
| CFA | `CFARepeatPatternDim` 2x2, `CFAPlaneColor` 0,1,2, `CFALayout` 1; patterns RGGB, BGGR, GRBG, GBRG | X-Trans, 4-color and other layouts are `unsupported-capability` |
| `BitsPerSample` | integer 8–16 (packed MSB-first per DNG for 10/12/14), float 16/24/32 for LinearRaw | `unsupported-capability` |
| `Compression` | 1 none; 7 lossless JPEG (ITU T.81 process 14, SOF3, predictors 1–7, 1–4 components, restart markers); 8 deflate with `Predictor` 1, 2, 34894 or 34895 | 34892 lossy JPEG, 52546 JPEG XL and others are `unsupported-capability` |
| Layout | strips or tiles, `PlanarConfiguration` 1 | `unsupported-capability` |

**Tags honored:** `LinearizationTable`, `BlackLevel` (with `BlackLevelRepeatDim`),
`BlackLevelDeltaH/V`, `WhiteLevel`, `ActiveArea`, `MaskedAreas` (ignored),
`DefaultCropOrigin/Size`, `DefaultScale` (must be 1/1 in engine 1),
`BaselineExposure`, `Orientation`, `AsShotNeutral`, `AsShotWhiteXY`, `AnalogBalance`,
`ColorMatrix1–3`, `ForwardMatrix1–3`, `CameraCalibration1–3`, `CalibrationIlluminant1–3`,
`ProfileHueSatMapData1–3` with dims, `ProfileLookTableData` with dims,
`ProfileToneCurve`, `ProfileName`, `ProfileEmbedPolicy`, `UniqueCameraModel`, `Make`,
`Model`, the EXIF and GPS IFDs (for `capture` and privacy only), and `OpcodeList1–3`.

**Opcodes.** Lists are applied at the DNG-specified points (list 1 on stored raw
values, list 2 after linearization, list 3 after demosaic). Engine 1 implements
`WarpRectilinear` (1), `FixVignetteRadial` (3), `FixBadPixelsConstant` (4),
`FixBadPixelsList` (5), `TrimBounds` (6), `MapTable` (7), `MapPolynomial` (8) and
`GainMap` (9). An unknown opcode that is flagged optional or preview-only is skipped,
and inspection lists it under `raw.opcodes` with `"applied": false`. An unknown
mandatory opcode lets the source be inspected, but developing it fails with
`[unsupported-capability]`.

**Embedded previews** (`NewSubfileType = 1`) are never exported. The grid and loupe
may show one, labeled `embedded-preview`, only until a developed render is cached.

**Limits (checked in this order, before allocation):** the file is at most 512 MiB;
at most 32 IFDs in total with SubIFD depth up to 2, and cycles are rejected; at most
1024 entries per IFD, each with its value inside the file; each side is at most
32768 and the image at most 120 megapixels (`MAX_PHOTO_PIXELS`); at most 65536
strips or tiles; every strip or tile offset and byte count lies inside the file; `LinearizationTable` up to 65536 entries; opcode lists up to 16 MiB
and 256 opcodes, a bad-pixel list up to 1,000,000 entries, gain maps up to 1,048,576
points; profile LUTs up to 1,048,576 entries. Each compressed strip or tile must
decode to exactly its expected size; output beyond that is `[malformed-resource]`,
which makes decompression bombs impossible. The decoder checks cancellation once
per strip or tile.

Error codes: structural damage is `malformed-resource`, limits are
`limit-exceeded`, unsupported features are `unsupported-capability`. Every message
names the tag, IFD, or feature and suggests a corrective action.

## Working color and precision

- **Working space:** `prophoto-linear`, which is ROMM/ProPhoto primaries with a D50
  white and a linear transfer, stored as `f32`. Values are scene-referred and
  unclamped: negatives and values above 1.0 survive every stage until output. D50
  is the DNG profile connection space, so camera matrices need no further
  adaptation.
- **Storage precisions:** `u8` and `u16` for sources and outputs, `f16` and `f32`
  for derived (merge) sources. Conversions (exact rules in
  [Working representation](#working-representation)):
  - u8 and u16 to f32: `transfer.decode(c / max)` from a table, then the matrix;
  - f32 to u16: the matrix, clip to [0, 1], then round half up in the encoded
    domain, `floor(encode(v) * 65535 + 0.5)`;
  - f32 to u8: the same, or with optional `dither: "ordered4"`.
- **No hidden 8-bit round trips.** No stage between decode and output quantizes to
  8 bits or to sRGB. The page compositor receives a `photo` node's pixels converted
  once from the working space to `srgb8`. A 16-bit wide-gamut export never passes
  through `srgb8`.
- **Chromatic adaptation** uses the Bradford matrix. The profile connection space is
  CIE XYZ with a D50 white.

### RGB color spaces

| Id | Primaries (x, y) R; G; B | White | Transfer |
|---|---|---|---|
| `srgb` | 0.640,0.330; 0.300,0.600; 0.150,0.060 | D65 | sRGB piecewise |
| `display-p3` | 0.680,0.320; 0.265,0.690; 0.150,0.060 | D65 | sRGB piecewise |
| `adobe-rgb-1998` | 0.640,0.330; 0.210,0.710; 0.150,0.060 | D65 | gamma 563/256 |
| `prophoto` | 0.7347,0.2653; 0.1596,0.8404; 0.0366,0.0001 | D50 | ROMM gamma 1.8 (linear `16x` below 1/512) |
| `rec2020` | 0.708,0.292; 0.170,0.797; 0.131,0.046 | D65 | BT.709 OETF (SDR), `pq` or `hlg` (HDR) |

D65 is (0.3127, 0.3290) and D50 is (0.3457, 0.3585). Every space may also be given
with an explicit transfer as `<id>:linear`. Output ICC profiles are generated by
pentool as deterministic matrix/TRC ICC v4 profiles, so no profile file is needed at
runtime.

HDR transfers are not color space ids. A recipe's `hdr.transfer` (`pq` or `hlg`)
replaces the BT.709 OETF of `rec2020`.

### Working representation

Implemented in `src/photo/` (item 2).

- **Transfers.** Constants:
  - sRGB: 0.04045, 12.92, 0.055, 2.4 and 0.0031308.
  - Adobe RGB: pure power 563/256.
  - ROMM: `16 L` below `L = 1/512`, `L^(1/1.8)` above.
  - BT.709/2020: α = 1.09929682680944, β = 0.018053968510807, `4.5 L` below β.
  - PQ: ST 2084 m1, m2, c1, c2 and c3, where linear 1.0 is 10,000 cd/m². Exact
    zero encodes to 0, not `c1^m2`.
  - HLG: BT.2100, a = 0.17883277.

  Negative inputs mirror: `f(-x) = -f(x)`.
- **Arithmetic.** `pow`, `log2` and `exp2` are pentool's own (`photo::math`).
  They use only IEEE `+ - * /`, `floor` and bit operations, so every platform
  produces identical bits. Platform `libm` is never called.
- **Matrices.** Each RGB to XYZ matrix is derived in `f64` from the table's xy
  values. The derived ProPhoto Z row is therefore 0.8251, not ISO 22028-2's
  0.8249, which uses the ICC XYZ white. Each space converts to the working space
  as `XYZ_D50→ProPhoto · Bradford(white→D50) · RGB→XYZ`. Every named space's
  white maps to (1, 1, 1) within 1e-12.
- **Decode.** Code `c` of `max` (255 or 65535) becomes
  `transfer.decode(c / max)` from an `f64` table. The `f64` matrix to the working
  space is then applied, and only the result is rounded to `f32`.
- **Encode.** Working `f32` values go through the `f64` matrix to linear output
  RGB, `v`. `v` is clipped to [0, 1] (a relative colorimetric clip; `photo render` maps
  the gamut first, see "Wide-gamut and HDR delivery"). NaN counts as 0. The code is the count of
  `c` in `1..=max` with `decode((c - t) / max) <= v`. That is round half up of
  `encode(v)` with offset `t = 0.5`, computed by binary search over `f64`
  thresholds with no per-pixel `pow`.
- **Dither.** `ordered4` uses `t = (B[y mod 4][x mod 4] + 0.5) / 16` with the
  Bayer matrix `[[0,8,2,10],[12,4,14,6],[3,11,1,9],[15,7,13,5]]`. It is valid
  for 8-bit output only; 16-bit output with it is `[unsupported-capability]`.
- **Alpha.** Alpha is straight, linear and unaffected by color conversion.
  Decode is `c / max`; encode is `floor(clamp(a, 0, 1) * max + 0.5)`.
- **Clip report.** A pixel counts as clipped when any channel's `v` lies outside
  `[decode(-0.5 / max), decode((max + 0.5) / max)]`, which is exactly when
  clipping changes a code.
- **Limits.** Before allocation, each side must be 1–32768, the pixel count at
  most `MAX_PHOTO_PIXELS`, and the working buffer (12 bytes per pixel, 16 with
  alpha) at most 2 GiB.
- **PNG.** 8- and 16-bit PNG are read and written through the existing codec.
  Gray and gray-alpha expand losslessly to RGB(A). Orientation is reported, not
  applied. Delivered PNGs carry color chunks; see "Wide-gamut and HDR delivery".
- **Measured precision.** The fixture is `photo-rgb16-p3.png`; tests are in
  `tests/v0110_photo_precision.rs` and `src/photo/pixels.rs`.
  - Bit-exact 16-bit round trips through the working space, for every code on
    every channel: `srgb`, `display-p3`, `prophoto`, `rec2020` and any `:linear`.
    So are 8-bit round trips in every named space, and alpha.
  - `adobe-rgb-1998` has no linear toe. Next to a bright channel, near-black
    codes may move by up to 12 of 65535. The linear-light change stays within
    2^-24, one `f32` ulp at 1.0.
  - Display P3 through 16-bit `prophoto` storage and back: at most 9 codes, and a
    linear-light error within half a ProPhoto step at white times the matrix row
    gain, plus half a P3 step.
  - After D65→D50 adaptation, Display P3 red needs ProPhoto blue −0.00127, so
    saturated P3 (and Rec. 2020) reds and cyans fall outside [0, 1] ProPhoto
    storage. The unclamped working space holds them. A 16-bit `prophoto` file
    clips them and the clip report counts them.

## Development pipeline

A variant's `develop` object fully describes its appearance. `develop.process` is
the process version (1 for engine 1). A build renders and edits any process version
up to its own. A newer process is view-only: the cached render can be shown, but
editing or re-rendering fails with `[unsupported-capability]`. A process version is
never reinterpreted: changing a formula or a curve requires a new process version.

### Processing order (process 1)

The stage order is fixed and cannot be changed by the user. This is what "stable
processing order" means. Local adjustments add per-pixel deltas within the stages
they list.

| # | Stage | Settings groups |
|---|---|---|
| 1 | Decode: unpack, opcode list 1, linearize, black and white levels, opcode list 2, defective pixels, raw white-balance multipliers, highlight handling, demosaic, opcode list 3, crop to `ActiveArea`/default crop | `raw`, `white_balance` |
| 2 | Early detail on linear camera RGB: color and luminance noise reduction, moiré, defringe | `detail.noise`, `detail.moire`, `lens.defringe` |
| 3 | Camera profile: camera RGB to XYZ D50 to working space, calibration, profile hue/sat map | `raw.camera_profile`, `calibration` |
| 4 | Lens: lateral chromatic aberration, distortion, vignetting | `lens` |
| 5 | Geometry: upright/perspective, rotation, scale, offset, then crop. Stages 4–5 are resampled once, as one composed warp | `geometry`, `crop` |
| 6 | Tone: exposure (plus `BaselineExposure`), contrast, highlights, shadows, whites, blacks | `tone` |
| 7 | Presence: dehaze, clarity, texture | `presence` |
| 8 | Curves: parametric, then point (master, then R, G, B) | `curves` |
| 9 | Color: monochrome mix or HSL, then vibrance, saturation, color grading | `hsl`, `grading`, `presence`, `monochrome` |
| 10 | Effects: post-crop vignette, then grain (seeded) | `effects` |
| 11 | Output rendering: profile look table and tone curve, SDR shoulder, gamut mapping to the output space | `output` (from the node or recipe) |
| 12 | Output: resize, output sharpening, transfer function, quantization | recipe |

Capture sharpening (`detail.sharpening`) runs at the end of stage 9 on luminance.
Output sharpening is a separate recipe setting in stage 12.

Rendered (non-raw) sources skip the raw parts of stage 1. Their input profile
replaces stage 3. White balance on a rendered source is applied in stage 3 as a
relative Bradford shift: `temperature` and `tint` there are -100..100 offsets, not
kelvin.

**Spatial scale.** Every radius is defined as a fraction of the long edge of the
developed, uncropped image. Proxies therefore approximate full-resolution output.
Only full-resolution output is golden-tested. Statistics that a stage needs
globally (dehaze airlight, auto tone) are computed by commands from a fixed
1024-pixel-long-edge analysis proxy and stored as resolved values.

**Tiling.** Stages run on 512x512 tiles. Each tile carries an apron that covers the
stage's neighborhood, and tiles may be processed in parallel. A full f32 frame is
never required: the peak memory estimate must stay within 2 GiB (`MAX_DEVELOP_BYTES`,
`limit-exceeded`) and is checked before decoding.

Numerical kernels (demosaic weights, curve shapes, tone operators, noise filters)
are documented in this file, in the section of the roadmap item that implements
them, before that item is ticked. Once released under process 1, they never change.

### Development settings (`develop`)

Every group is optional, and an absent group or parameter means identity (no
change). Unknown groups or parameters, values out of range, and parameters given in
a context that does not use them are `[invalid-develop]`. Ranges are inclusive.

| Group | Parameters (range; identity) |
|---|---|
| `raw` | `demosaic` `bilinear`\|`mhc` (Malvar–He–Cutler; default `mhc`); `highlights` `clip`\|`blend`; `camera_profile` `"embedded"`\|`"matrix-only"`\|`{"profile": digest}`; `defective_pixels` `{auto: bool, threshold 1–100, list: [[x,y]...] ≤ 4096}` |
| `white_balance` | `{mode: "as-shot"}`; `{mode: "temperature", temperature 2000–50000 K, tint -150–150}` (raw); `{mode: "relative", temperature -100–100, tint -100–100}` (rendered); `{mode: "neutral", neutral: [r,g,b] > 0, sampled?: {x, y, radius}}`. Optional `auto` provenance |
| `tone` | `exposure` -5–5 EV; `contrast`, `highlights`, `shadows`, `whites`, `blacks` -100–100 |
| `presence` | `texture`, `clarity`, `dehaze`, `vibrance`, `saturation` -100–100 |
| `curves` | `parametric {highlights, lights, darks, shadows -100–100; splits [3 increasing in 0.05–0.95]}`; `point {rgb, red, green, blue: 2–16 [in,out] points in 0–1, strictly increasing in}` |
| `hsl` | `hue`, `saturation`, `luminance`: each maps the bands `red orange yellow green aqua blue purple magenta` to -100–100 |
| `grading` | `shadows`, `midtones`, `highlights`, `global`: `{hue 0–360, saturation 0–100, luminance -100–100}`; `blending` 0–100 (50); `balance` -100–100 |
| `monochrome` | `enabled` bool; `mix`: the 8 bands -100–100 |
| `detail` | `sharpening {amount 0–150, radius 0.5–3, detail 0–100, masking 0–100}`; `noise {luminance, luminance_detail, luminance_contrast, color, color_detail, color_smoothness: 0–100}`; `moire` 0–100 |
| `lens` | `profile` `"none"`\|`"embedded-opcodes"`\|`{"profile": digest}`; `distortion` -100–100; `vignetting {amount -100–100, midpoint 0–100}`; `chromatic_aberration {remove: bool, red_cyan -100–100, blue_yellow -100–100}`; `defringe {purple_amount 0–20, purple_hue [30–70, 30–70], green_amount 0–20, green_hue [40–60, 40–60]}` |
| `geometry` | `upright` `off`\|`level`\|`vertical`\|`full`\|`guided` (an auto mode stores its resolved values plus `auto`); `guides` ≤ 4 line segments (guided only); `vertical`, `horizontal` -100–100; `rotate` -45–45°; `aspect` -100–100; `scale` 50–150 (100); `offset` [-100–100, -100–100] |
| `crop` | `rect` [x, y, w, h] normalized to the post-geometry frame; `aspect` `"free"`\|`"original"`\|`"W:H"`; `constrain` bool (stay inside valid pixels) |
| `effects` | `vignette {amount -100–100, midpoint, roundness -100–100, feather 0–100, highlights 0–100}`; `grain {amount 0–100, size 0–100, roughness 0–100, seed u32}` |
| `calibration` | `shadows_tint` -100–100; `red`, `green`, `blue`: `{hue, saturation -100–100}` |
| `local` | up to 32 local adjustments (below) |

**Import defaults** are written explicitly into the master variant by `raw add` and
`photo import`. For raw sources these are `raw.demosaic: "mhc"`,
`raw.highlights: "blend"`, `raw.camera_profile: "embedded"`,
`white_balance.mode: "as-shot"`, `lens.profile: "embedded-opcodes"`,
`detail.sharpening {amount 40, radius 1, detail 25}` and `detail.noise.color 25`.
Rendered sources get no defaults. Later builds may suggest different defaults for new
imports, but they never change the meaning of settings that are already stored.

### Camera profiles and white balance

- No camera database exists. `--camera-profile auto` selects the profile embedded
  in the DNG. DNG requires `ColorMatrix1`, so every accepted raw has one. A matrix
  is interpolated by correlated color temperature between illuminants as the DNG
  specification describes: Robertson's method, a fixed iteration limit of 30, and a
  tolerance of 1e-6 in xy.
- Verified profiles are DNG camera profiles (`.dcp`), imported with
  `photo profile add` into `photography.profiles` (digest, `kind: "camera"`, `name`,
  `unique_camera_model`, `embed_policy`). They are parsed with the same bounded
  TIFF reader. A profile is verified when it decodes, passes the limits, has a
  digest that matches, and has a `UniqueCameraModel` that matches the source. A
  mismatch is refused unless `--force-model` is given, and that choice is recorded.
- White balance modes: `as-shot` (`AsShotNeutral` or `AsShotWhiteXY`); temperature
  and tint (xy through Robertson; one tint unit is 1/3000 in uv, the DNG SDK's
  scale); a sampled `neutral` (the command samples a radius in camera RGB and
  stores the neutral); and a suggestion (deterministic gray-world with clipped-pixel
  rejection on the analysis proxy, stored as temperature and tint with
  `auto: {"algorithm": "gray-world", "version": 1}`).

**Profile reading.** A profile carries 1–3 calibrations (`ColorMatrixN`, required for
N = 1; `CalibrationIlluminantN`; optional `ForwardMatrixN` and
`ProfileHueSatMapDataN` with `ProfileHueSatMapDims` and `ProfileHueSatMapEncoding`
0 or 1). Forward matrices must be present for all calibrations or for none. A
matrix whose determinant is below 1e-9, or whose values exceed 1e4 in magnitude,
is `[malformed-resource]`. Color matrices are normalized so that the D50 white maps
to a camera neutral with a maximum of 1 (`NormalizeColorMatrix`). Forward matrices
are normalized so that a unit camera neutral maps to D50 XYZ
(`NormalizeForwardMatrix`). A hue/sat map holds at most 1,048,576 entries. An
identity map counts as absent; when only some calibrations have a map, the others
get an identity map of the same dimensions. Maps that differ in dimensions are
`[malformed-resource]`. `matrix-only` uses the embedded profile without forward
matrices or hue/sat maps. A `.dcp` must carry `UniqueCameraModel`, and is at most
16 MiB.

**Illuminant temperatures.** EXIF light sources map to kelvin as follows: 17 and 3
→ 2850; 24 → 3200; 23 → 5000; 20, 1, 9, 4 and 18 → 5500; 21, 19 and 10 → 6500; 22
and 11 → 7500; 12 → 6400; 13 → 5050; 14 and 2 → 4150; 15 → 3525; 16 → 2925. Any
other code is unknown.

**Binding a profile to a source.** `AnalogBalance` (AB) and `CameraCalibrationN`
(CC) come from the DNG. CC applies to the embedded and matrix-only profiles. It
applies to an imported profile only when the DNG's `CameraCalibrationSignature`
equals the profile's `ProfileCalibrationSignature`; otherwise CC is the identity.
Each calibration contributes `AB·CC·CM`.

**Interpolation (`FindXYZtoCamera`).** For a white xy, the temperature is found by
Robertson's method. Calibration 1 is used alone when the profile has fewer than two
calibrations, when any temperature is unknown, or when two temperatures are equal.
Otherwise calibrations are sorted by temperature and the bracketing pair (T1 < T2)
is blended. The weight of the lower-temperature calibration is
`g = (1/T − 1/T2) / (1/T1 − 1/T2)`, clamped to 0–1. Color matrices, camera
calibrations, forward matrices and hue/sat map entries are blended linearly with
`g`.

**Neutral to white (`NeutralToXY`).** Start from D50. Each pass takes
`xyz = inverse(AB·CC·CM(last)) · neutral` and its chromaticity. Stop when x and y
both move less than 1e-6. The last of 30 passes averages the previous and the new
xy.

**White to transform (`SetWhiteXY`).** The camera white is `CM(xy) · XYZ(xy)`,
normalized to a maximum of 1 and clamped to 0.001–1. With forward matrices,
camera-to-PCS is `FM · inverse(diag(inverse(AB·CC) · white)) · inverse(AB·CC)`.
Without them it is the inverse of `CM · Bradford(D50 → xy)`, scaled so that D50 maps
to a camera maximum of 1. Stage 1 balances camera RGB with the multipliers
`max(white) / white`. Stage 3 then applies
`inverse(ProPhoto→XYZ) · CameraToPCS · diag(white)`, which maps a balanced
(1, 1, 1) exactly to the working white (1, 1, 1).

**Hue/sat map.** The map is applied in linear ProPhoto after the matrix, in DNG HSV
(hue 0–6). Lookup is trilinear, or bilinear in hue and saturation when the map has
one value division. With encoding 1, only the lookup coordinate uses the sRGB
transfer of `min(v, 1)`. The result is `h += shift · 6/360`,
`s = min(s · scale, 1)`, and `v = v · scale` (not clamped). Pixels with a negative
channel or `v ≤ 0` pass through unchanged.

**Measured white balance.** `--sample x,y,radius` takes a centre in the oriented
frame (0–1) and a radius as a fraction of the long edge (at least 0.5 px). It
decodes camera RGB with a unity neutral and clipped highlights, averages the disk
while excluding pixels with any channel ≥ 0.98, and stores the average normalized to
a maximum of 1, rounded to 6 decimals. `--suggest` decodes the same way. On a grid
of blocks with a step of `ceil(long edge / 1024)`, it rejects blocks with any
channel ≥ 0.98 and blocks whose mean maximum is below 0.002, averages the remaining
block means in f64, and stores the resulting white as kelvin and tint. These are
clamped to 2000–50000 K and ±150, rounded to integers, and stored with `auto`
provenance. A monochrome DNG (LinearRaw with one sample) uses the identity
transform; sampling, suggesting and a `neutral` are `[invalid-develop]` for it.

**Rendered sources.** `relative` white balance is a working-space Bradford
adaptation from the white at `200 − temperature` mired with `tint`, to the white at
200 mired (5000 K) with tint 0. Zero is exactly the identity.

### Lens and geometry correction

There is no lens database. Sources of correction:

1. the DNG's own opcodes (`WarpRectilinear`, `FixVignetteRadial`), selected with
   `lens.profile: "embedded-opcodes"`;
2. pentool lens profiles (`photo profile add --lens FILE.json`), with
   `pentool_lens_profile: 1`, `make`, `model`, the focal length and aperture
   ranges, and a set of samples. Each sample holds radial distortion coefficients
   k1–k3 with optional tangential terms (the `WarpRectilinear` model), a vignette
   polynomial (the `FixVignetteRadial` model) and lateral CA scales for R and B.
   Samples are interpolated linearly in focal length, then in 1/aperture;
3. opt-in `photo profile import-lcp FILE` converts a bounded subset of Adobe LCP:
   only the rectilinear distortion, vignette and lateral CA models are converted.
   Every other model is listed under `unsupported` and not approximated, in the same
   way as `preset-import-mypaint`;
4. the manual parameters in `lens` and `geometry`.

Stages 4–5 compose into one inverse mapping, which is sampled once with
Catmull–Rom bicubic interpolation in linear light. `crop.constrain` restricts the
crop to the largest rectangle with the crop's aspect that contains no invalid
(outside-source) pixels. Without it, invalid pixels are transparent and are reported
by `photo info`.

**Lens profile format.** A pentool lens profile is a JSON object with exactly
`pentool_lens_profile` (1), `make` and `model` (1–128 characters; the profile's
`name` is `"make model"`), `focal_range` and `aperture_range` ([min, max], min ≤
max), and 1–1024 `samples`. A sample holds `focal` and `aperture` (inside the
ranges; each pair at most once), optional `scale` (the distortion radius unit as a
fraction of the long edge, 0–10; absent is the corner distance) and `center`
([x, y] in long-edge units from the top-left; absent is the image center),
`distortion` (`radial` k1–k3 in ±10, `tangential` t0–t1 in ±1),
`vignette` (exactly one of `gain` k0–k4, the `FixVignetteRadial` polynomial, or
`falloff` a1–a3, the LCP model whose gain is the inverse of
`1 + a1 r² + a2 r⁴ + a3 r⁶`; each in ±100, with its own optional `scale` and
`center`), and `chromatic_aberration` (`red`, `blue` radial scales relative to
green, 0.9–1.1, default 1). Every sample must have the same shape. A profile is
selected for a capture's EXIF focal length and FNumber (both required; otherwise
`[invalid-develop]`): coefficients are interpolated linearly in focal length
between the two bracketing focal lengths, then in 1/aperture, each clamped to the
calibrated range. The catalog record is `kind: "lens"`, `name`,
`imported_from` (`pentool-lens` or `lcp`), `byte_length`, `storage`, and for LCP
the `unsupported` list (at most 256 strings).

**LCP subset.** `photo profile import-lcp` reads the XML with a bounded parser: a
`DOCTYPE` (and so any entity) is `[unsupported-capability]`; at most 200,000
elements and a depth of 64. Each `stCamera` profile converts to one sample:
`FocalLength`; `ApertureValue` (APEX, N = 2^(AV/2)) or `FNumber`; and when a
focal/aperture pair repeats at several focus distances, the farthest is kept.
`PerspectiveModel` converts `FocalLengthX` to `scale`, `ImageXCenter` and
`ImageYCenter` to `center`, `RadialDistortParam1–3` and `TangentialDistortParam1–2`.
Its `VignetteModel` converts `VignetteModelParam1–3` to `falloff`, and
`ChromaticRedGreenModel` and `ChromaticBlueGreenModel` convert `ScaleFactor` to
the red and blue scales. Every other model or parameter (a fisheye model, a
nonzero chromatic distortion term, and so on) is named in `unsupported`; nothing is
approximated. The stored bytes are the canonical pentool lens profile (sorted,
pretty JSON with a final newline), so importing the same LCP twice deduplicates.

**Frames.** The lens chain works in the decoded (default-cropped, unoriented)
frame. Opcode list 1 positions are in stored-image pixels and lists 2 and 3 in
active-area pixels; both are translated into the decoded frame. `WarpRectilinear`
and `FixVignetteRadial` opcodes are applied here, not during decoding. Geometry,
guides, crop and the white-balance sample point use the oriented frame after lens
correction.

**Lens chain.** In forward (optical) order the steps are: manual chromatic
aberration, the profile or opcode steps, manual distortion, manual vignetting.
Radii are measured from the center and divided by the distance to the farthest
corner unless a profile gives a scale. The output is sampled through the inverse:
the steps are walked in reverse, a warp evaluating the `WarpRectilinear` inverse
map per channel plane and a vignette multiplying by its gain at the current
position.

- `chromatic_aberration.red_cyan` and `blue_yellow` (±100) scale the red and blue
  planes radially by `1 + v/20000`. A profile's CA scales apply only with
  `chromatic_aberration.remove: true`; otherwise every channel uses the green plane.
- `distortion` d (±100; positive removes barrel) is a radial warp with k1 = −d/400.
- `vignetting.amount` a (±100) and `midpoint` m (0–100, default 50) multiply by
  `2^(a/100 · r^q)` with q = 1 + 4m/100.
- A profile's vignette is applied before its warp, with its CA folded into the warp
  planes.

**Geometry.** Coordinates are centered on the oriented frame and divided by half
its long edge. The forward transform is `Offset · Scale · Aspect · Rotate ·
Perspective`:

- Perspective is the identity with its third row `[−0.5·h/100, 0.5·v/100, 1]`, for
  `horizontal` h and `vertical` v.
- `rotate` is in degrees.
- `aspect` a stretches x by 2^(a/200) and y by its inverse.
- `scale` is in percent.
- `offset` [x, y] moves by x/100 of the half width and y/100 of the half height.

The inverse transform maps each output pixel center back. A homogeneous w at or
below 1e-12 marks the pixel invalid. The lens chain inverse is then applied, then the
orientation is undone. A pixel is valid when every channel's source position lies
inside [0, W] × [0, H].

**Crop.** `rect` is normalized to the post-geometry oriented frame and snapped to
whole pixels by rounding; absent is the whole frame. The schema bounds each value
to 0–1. Validation is stricter: w and h must be positive, and the rectangle must
lie inside the frame. `aspect` is used only by `constrain`. With `constrain`, the
crop is reduced to the largest rectangle of that aspect centered in `rect`
(`free` keeps the rect's aspect, `original` the frame's). The reduction is a
30-step binary search on the scale, with 256 validity samples per edge, snapped
inward to whole pixels. A crop whose center has no source is `[invalid-develop]`.

**Upright.** The analysis runs on the lens-corrected oriented image with identity
geometry. The image is box-downsampled to at most 1024 px on the long edge, as the
square root of luminance.

1. Sobel gradients feed a Hough transform: angles within ±25° of vertical and of
   horizontal in 0.25° steps, and pixels vote only when their gradient direction
   agrees within ±10°.
2. Non-maximum suppression keeps at most 16 peaks per family, and each peak
   becomes a segment between the extreme projections of its voting pixels.
3. The cost is the sum over segments of length · min(deviation°, 10)².
   - `level` solves `rotate`.
   - `vertical` solves `rotate` and `vertical`.
   - `full` solves `rotate`, `vertical` and `horizontal`.
   - `guided` uses 1–4 user segments instead of detected lines: `--guide
     x1,y1,x2,y2` in the lens-corrected oriented frame (0–1). Segments steeper
     than 45° are vertical. It solves `rotate`, plus `vertical` with two or more
     vertical guides and `horizontal` with two or more horizontal guides.
   In the cost, a perspective point whose w is at or below 1e-6 adds the maximum
   deviation.
4. Coordinate descent runs three cycles. Steps are 0.5° for rotation (±25) and 2
   for perspective (±100), refined by /10 and /100.
5. Results are rounded to 0.01° and 0.1. The stored values replace `rotate`,
   `vertical` and `horizontal` (unsolved keys are removed), with
   `auto: {"algorithm": "upright-hough"|"upright-guided", "version": 1}`.

No lines is `[invalid-input]`. `--upright off` removes upright, `auto`, guides,
rotation and perspective.

### Development stack (stages 6–10 and calibration)

These kernels are process 1 (`src/photo/adjust.rs`). They use only `+ - * /`,
`sqrt`, and the in-tree `log2`, `exp2`, `pow`, `sin_cos` and `atan2`.

An absent group, or one whose values are all at identity, skips its kernel, so
the pixels pass through bit for bit. Sliders are written `v` below and
normalized to `v / 100`. Every row is computed independently, so the thread
count never changes a result.

Notation:

- `Y` is working luminance, the Y row of linear ProPhoto to XYZ D50.
- `s = log2(max(Y, 0.18 · 2⁻²⁰) / 0.18)` is stops from middle gray.
- `bump(s, c, w) = (1 − t²)²` for `t = (s − c) / w` with `|t| < 1`, and 0
  otherwise.
- `S(t)` is smoothstep `3t² − 2t³` with `t` clamped to 0–1.
- `L` is the long edge, in pixels, of the developed, uncropped frame.

**Blurs.** A Gaussian of sigma σ is three box passes per axis, using the
Kutskir box sizes for σ. Each pass keeps an `f64` running sum with
clamp-to-edge borders, and a σ below 0.5 px is skipped. When invalid pixels
exist, the blur is weighted: the blur of `value · alpha` divided by the blur
of `alpha`. The minimum filter is a van Herk/Gil–Werman square that ignores
samples outside the image.

**Calibration (stage 3).** For each primary `c` with hue `h` and saturation
`σ`:

1. Take the primary's offset from gray, `e_c − (⅓, ⅓, ⅓)`.
2. Rotate it about the gray axis by `20° · h`.
3. Scale it by `1 + 0.5σ`, then add it back to gray. The result is column `c`.

The rows are then normalized to sum to 1 so the working white stays neutral.
The matrix is composed after the camera matrix and before the profile's
hue/sat map. `shadows_tint` multiplies green by `2^(−0.25 · v · w)` after the
matrix. Here `w = (1 − Y / 0.05)²` below `Y = 0.05`, and `w = 1` at or below
0.

**Tone (stage 6).** The gain is `g = 2^(exposure + BaselineExposure)`, and `s`
is measured after the gain. The curve is:

```text
T(s) = s · (1 + 0.6 · contrast)
     + shadows · bump(s, −3, 3)
     + highlights · bump(s, 1.2, 2.2)
     + 0.6 · whites · bump(s, 2.5, 1.5)
     + 0.6 · blacks · bump(s, −6.5, 3.5)
```

Each pixel's RGB is multiplied by `g · 2^(T(s) − s)`. This preserves the
channel ratios, so tone never shifts hue. A pixel whose `Y` is not positive or
not finite gets `g` alone. Highlights and shadows are global, not local, in
process 1.

**Presence (stage 7)** applies dehaze, then clarity, then texture.

- **Dehaze** needs the resolved `presence.dehaze_airlight`, `A`:
  - For `d > 0`:
    1. The dark channel is `min_c max(I_c, 0) / A_c`.
    2. Min-filter it with radius `r = max(1, round(0.006 L))`.
    3. Blur it with σ = r.
    4. Then `t = max(1 − 0.95 d · dark, 0.1)` and `J = (I − A) / t + A`.
  - For `d < 0`, `t = 1 + 0.6d` and `J = I · t + A(1 − t)`.
- **Clarity** replaces `s` by `s + 0.6c (s − base) · bump(s, −1, 7)`, where
  `base` is `s` blurred with σ = `0.015 L`.
- **Texture** replaces `s` by `s + 0.8t (s − blur(s))`, with σ =
  `max(0.8, 0.002 L)`.

The pixel is multiplied by `2^(Δs)` when `Y > 0`.

**Curves (stage 8).** Each channel's composed curve is sampled at 4097 points
of the ROMM-encoded (ProPhoto) value: `F_c = point_c ∘ point_rgb ∘ parametric`.
Samples are linearly interpolated.

- The **parametric** curve is
  `x + (1 − (1 − 2x)⁸) · Σ 0.15 a_k · bump(x, center_k, width_k)` over the
  shadows, darks, lights and highlights regions. The regions are bounded by
  0, the three `splits` (default 0.25, 0.5, 0.75) and 1. The result is clamped
  to 0–1 and then made non-decreasing.
- **Point** curves are Fritsch–Carlson monotone cubics, flat outside their end
  points and clamped to 0–1.
- Linear values above 1 are scaled by `F(1)`, and values below 0 are offset
  from `F(0)`.

**Color (stage 9)** works in Oklab on the working space. The conversion is
Bradford D50→D65, then the Oklab M1 with its rows scaled so the D65 white is
exactly (1, 1, 1), then a cube root and M2.

Hue bands have centers red 20°, orange 55°, yellow 100°, green 140°, aqua 195°,
blue 255°, purple 295° and magenta 335°. A hue between two adjacent centers
weights them `1 − S(t)` and `S(t)`. The chroma weight is `q = C² / (C² + 0.0004)`,
so near-neutrals ignore luminance shifts.

1. **Monochrome** (when `enabled`): `L ← L · 2^(mix · q / 3)`, and `a = b = 0`.
   HSL, vibrance and saturation are skipped.
2. **HSL**, otherwise: hue rotates by `30° · hue`, `C ← C · max(0, 1 + saturation)`
   and `L ← L · 2^(luminance · q / 3)`.
3. **Vibrance:** `C ← C · (1 + v (1 − min(1, C / 0.3)))`. For `v > 0` the
   change is halved on the orange band to protect skin.
4. **Saturation:** `C ← C · (1 + s)`.
5. **Grading** comes last. With `x = clamp(L, 0, 1)`, `m = 0.5 − 0.25 · balance`
   and `w = 0.1 + 0.4 · blending`, the weights are:
   - shadows `1 − S((x − m + 0.25) / w + 0.5)`;
   - highlights `S((x − m − 0.25) / w + 0.5)`;
   - midtones the remainder, at least 0;
   - global 1.

   Each wheel adds `0.06 · saturation · (cos hue, sin hue)` to `(a, b)` and
   `0.1 · luminance` to `L`.

**Effects (stage 10)** run on the cropped output.

- The **vignette** works on `u, v`, the pixel's offsets from the center
  normalized to the half sizes.
  - With `roundness ρ > 0`, `u, v` are scaled toward a circle on the long edge.
  - With `ρ < 0`, the distance is a superellipse with exponent `2 − 6ρ`.
  - The gain is `2^(2a · S((d − d0) / f))`, with `d0 = 0.2 + 0.8 · midpoint`
    and `f = 0.05 + 0.95 · feather` (both default 50).
  - A darkening vignette fades on bright pixels by `highlights · S(Y − 0.5)`.
- **Grain** is value-lattice noise in uncropped-frame coordinates, so it does
  not move with the crop.
  - The cell is `max(1, (0.0005 + 0.002 · size) · L)` px, with smoothstep
    bilinear interpolation.
  - The lattice is blended with per-pixel noise by `roughness`.
  - The noise is the sum of four 16-bit uniforms from a splitmix64 hash of
    `(seed, x, y)`, centered and scaled to unit variance.
  - The pixel is multiplied by `max(0, 1 + 0.3 · amount · n · 4e(1 − e))`,
    where `e = Y / (Y + 0.18)`.
  - Size defaults to 25 and roughness to 50.

**Analysis.** Auto tone and the dehaze airlight read a proxy of stages 1–5 of
the whole frame, box-averaged (valid pixels only) to a long edge of at most
1024. `raw develop` stores the results, and rendering never re-analyzes.

- **Auto tone** (`raw develop --auto-tone`, `auto: {"algorithm": "auto-tone",
  "version": 1}`):
  - `exposure = clamp(−mean(s), ±5)`, rounded to 0.01, where `s` is measured
    with `BaselineExposure`.
  - If the 99th-percentile luminance `p` after that exposure exceeds 1,
    `highlights = −min(100, 40 log2 p)`.
  - `shadows = clamp(20 (−s₁ − 5), 0, 100)` from the first percentile `s₁`.
  - Both are rounded to integers, and zero values are omitted. Other tone
    values are kept.
  - Hand-editing a `tone` value removes `tone.auto`.
- **Airlight** (algorithm version 1) is resolved by `raw develop` whenever
  `presence.dehaze` is not 0 and no airlight is stored. It is resolved again
  when `raw`, `white_balance`, `calibration`, `lens`, `geometry` or `tone`
  changed in the same command, or when `--unset presence.dehaze_airlight` is
  given.
  1. Apply stage 6 to the proxy.
  2. Min-filter the dark channel with radius `max(1, round(0.006 · proxy
     edge))`.
  3. Take the brightest 0.1% (at least one; ties by index) and average their
     colors.
  4. Each component is at least 1e-6 and rounded to 1e-6.

  Rendering a nonzero `dehaze` without a stored airlight is `[invalid-develop]`.

### Detail (stages 1, 2 and capture sharpening)

Detail works at the sensor's pixel scale, so its radii are in pixels of the
developed frame, not fractions of `L`. An absent control, or one at zero,
skips its kernel and leaves the pixels bit for bit. Blurs are the three-pass
box Gaussian of the development stack.

- **Defective pixels** (`raw.defective_pixels`, stage 1, after opcode list 2
  on the normalized active area):
  - `list` coordinates are active-area pixels, before the default crop and
    orientation. A point outside the active area fails the render with
    `[invalid-develop]`.
  - With `auto: true`, a sample is defective when it lies more than
    `gap = (101 − threshold) / 200` (normalized raw units; `threshold`
    defaults to 50) above the maximum or below the minimum of its neighbors,
    or is not finite.
  - Neighbors are the same-color samples of the 5x5 window of a CFA plane, or
    the same channel's 3x3 window of a LinearRaw plane.
  - Each defective sample becomes the median (mean of the two middle values
    when even) of its non-defective neighbors, or of all neighbors when every
    one is defective. Detection reads the original plane, so the result does
    not depend on order.
- **Stage 2** runs on balanced camera RGB at the decoded size, in Oklab of the
  profile matrix's linear ProPhoto (including calibration), and converts back
  through the inverse matrix. Steps run in this order:
  1. Luminance noise (`detail.noise.luminance` `s`): the self-guided filter
     (He et al.) of Oklab L with Gaussian windows of
     `σ = 1 + 2 (1 − luminance_detail/100)` px (detail defaults to 50) and
     `ε = (0.08 s/100)²`. Then `L = q + (luminance_contrast/200) (L − q)`.
  2. Color noise (`color` `c`): a and b move toward their Gaussian blur with
     `σ = (1 + 6c/100)(0.5 + color_smoothness/100)` (smoothness defaults to
     50), by `min(1, 2c/100) / (1 + 0.5 color_detail · |L − blur(L)|)`
     (color detail defaults to 50).
  3. Moiré (`detail.moire` `m`): a and b move toward their blur with
     `σ = 2 + 6m/100` by `m/100 · smoothstep(h / 0.02)`, where `h` is the sum
     of their absolute high-pass values.
  4. Defringe (`lens.defringe`), purple then green, with amount `k/20`:
     - Hue sliders map to Oklab hue: purple `300° + 3(v − 50)` (30–70 →
       240°–360°), green `140° + 3(v − 50)` (40–60 → 110°–170°). Defaults are
       the full ranges.
     - The hue weight is 1 inside the window and fades linearly to 0 over 10°
       outside it, around the circle.
     - The edge weight is `smoothstep(e / 0.02)`, where `e` is the blurred
       absolute luminance high-pass, both with `σ = 1 + k/4`.
     - a and b are scaled by `max(0, 1 − amount · hue · edge)`.
- **Capture sharpening** (`detail.sharpening`, the end of stage 9, after
  color and before effects) is an unsharp mask of `s = log2(max(Y, FLOOR))`,
  with invalid pixels left out of the blurs:
  - `d = s − blur(s)` with `σ = radius` px (default 1).
  - Halos are damped: `d' = d / (1 + |d|/t)` with
    `t = 0.05 + 0.95 detail/100` stops (detail defaults to 25).
  - With `masking` `M > 0`, `d'` is multiplied by
    `smoothstep((blur(|d|) − 0.25M/100) / (0.25M/100))`, which protects flat
    areas.
  - Each valid pixel with positive luminance has its RGB multiplied by
    `2^(amount/100 · d')`. Hue is kept, and because the mask works on
    log luminance, the result does not depend on exposure.

### Local adjustments

```json
{"id": "sky", "name": "Sky", "enabled": true, "amount": 1,
 "mask": {"components": [
   {"kind": "linear", "mode": "add", "start": [0.5, 0.0], "end": [0.5, 0.45]},
   {"kind": "range-luminance", "mode": "intersect", "min": 0.55, "max": 1, "smoothness": 0.2}]},
 "params": {"exposure": -0.4, "dehaze": 15, "temperature": -8}}
```

- Mask coordinates are normalized to the developed frame after geometry and before
  crop. Changing the crop therefore never moves a mask.
- A mask has up to 16 components, combined in order. The first component may only
  use `mode: "add"`; later components use `add`, `subtract` or `intersect`. Every
  component may set `invert`.
- Component kinds:
  - `linear`: `start` and `end`; coverage ramps from 1 at `start` to 0 at `end`.
  - `radial`: `center`, `radius` [rx, ry], `angle`, `feather` 0–100, and `inside`
    (default true).
  - `range-luminance`: `min`, `max`, `smoothness` (perceptual Oklab lightness
    after stage 6).
  - `range-color`: `colors` (1–5 working-space samples), `amount` 0–100.
  - `depth`: `map` (the digest of a depth image), `min`, `max`, `smoothness`.
    Available only when the source pairs with a depth map imported explicitly.
  - `brush`: an 8-bit coverage plane of `width` x `height` (the uncropped frame
    scaled to a long edge of at most 4096). It is stored as `tiles` in document
    `raster_tiles`, exactly like raster selections. It is painted with the raster
    brush engine (`photo mask paint`), and its tiles are the source of truth.
  - `mask`: `resource`, the digest of a v6 `mask_resources` entry, with optional
    informational `provenance`. AI-derived masks enter only this way: they are
    first materialized as a reusable mask (v0.8/v0.9 paths) and then referenced.
    Rendering never contacts a provider.
- `params` may contain `exposure`, `contrast`, `highlights`, `shadows`, `whites`,
  `blacks`, `temperature`, `tint` (relative, -100–100), `texture`, `clarity`,
  `dehaze`, `hue` (-180–180), `saturation`, `sharpness`, `noise`, `moire`,
  `defringe` and `color` (`{hue, saturation}`). Each is added within its stage and
  weighted by coverage times `amount` (0–1).

Implementation (process 1):

- Coverage is computed once, after the global stage 6, at each pixel center in
  frame coordinates (`(origin + x + 0.5) / frame`), so a crop only changes which
  part of the frame is rendered. Components combine as add `t + c - tc`,
  subtract `t(1 - c)` and intersect `tc`; `invert` uses `1 - c` first. The final
  weight is coverage times `amount`. Disabled adjustments and an `amount` of 0
  are skipped.
- `linear`: `1 - smoothstep(t)`, with `t` the projection onto start→end. `start`
  and `end` must differ.
- `radial`: radii are fractions of the long edge (0 < r ≤ 4), rotated by `angle`;
  coverage is `smoothstep((1 - d) / feather)` with `d` the normalized elliptical
  distance and `feather` defaulting to 50. `inside: false` inverts it.
- `range-luminance`: on Oklab L (perceptual, 0–1), with linear ramps of width
  `smoothness` (default 0.1) outside `min`–`max`.
- `range-color`: Oklab distance `d = sqrt(0.25 ΔL² + Δa² + Δb²)` to the nearest
  sample; tolerance `T = 0.02 + 0.28 · amount / 100` (default amount 50); coverage
  `smoothstep(2 (1 - d / T))`.
- `depth` is refused with `[unsupported-capability]`: this build imports no depth
  maps.
- `brush`: bilinear sampling of the plane. Planes span at most 256 tiles and every
  tile digest must be in `raster_tiles`. `photo mask paint` paints into an
  existing brush component (`--component N`, whose size must match the current
  frame) or appends a new one; samples are in brush-plane pixels and `--erase`
  removes coverage. Tiles are retained by the raster garbage collector.
- `mask`: a raster `mask_resources` entry, read as luminance times alpha and
  sampled bilinearly over the frame. Vector masks are refused with
  `[unsupported-capability]`.
- Stage application: exposure, temperature and tint are per-pixel gains after the
  global tone (temperature and tint are luminance-preserving `2^(±0.3 w v)` channel
  gains); contrast, highlights, shadows, whites and blacks run the global tone curve
  on a copy and blend by the weight; dehaze, clarity and texture run the stage-7
  kernel at full strength on a copy and blend by `weight · |v| / 100` (a local
  dehaze uses the stored `presence.dehaze_airlight`, which `raw develop` resolves
  whenever a local dehaze exists); hue, saturation and `color` act in Oklab after
  stage 9; sharpness, noise, moire and defringe run after capture sharpening
  (negative sharpness is a σ 1.5 blur; negative noise, moire and defringe
  extrapolate away from the reduced image).
- The render report lists `local: [{id, coverage}]`, the mean weight of each
  applied adjustment.

### HDR and panorama merges

`photo merge-hdr` (2–9 inputs) and `photo merge-pano` (2–64 inputs, at most 1.2
gigapixels of input in total, counted from the recorded asset sizes before any
decode) build a new photo whose source is a derived DNG. They never modify the
inputs. Algorithm version 1 is implemented in `src/photo/merge.rs`.

- Inputs are raw or derived sources; rendered sources, and derived sources with
  transparency (panoramas), are refused. Each input is developed through stages
  1–3 only, from its master variant's `process`, `raw`, `white_balance`, `detail`
  (defective pixels and stage-2 detail) and `calibration`, so it is scene-linear
  working RGB. It is then turned upright by its orientation and, with `--scale`
  (0 < scale ≤ 1, default 1), box-averaged to the rounded scaled size. Each
  input is decoded twice (measure, then merge), so only one full-resolution
  input is in memory beside the measurements.
- HDR, all inputs must share one developed size.
  - Reference: `--reference N` (1-based), else the middle input
    (`(n-1)/2` after sorting by EXIF exposure when every input has one, else by
    median luminance).
  - Alignment: integer translation by median-threshold bitmaps over up to six
    half-size levels (side ≥ 16), with an exclusion band of 4% of the median, a
    3x3 search per level in a fixed order with (0,0) first, and an error of
    disagreeing bits over the overlap. `shift` `[sx, sy]` means input pixel
    `p + shift` sees reference pixel `p`.
  - Exposure: the factor relative to the reference is EXIF `t·ISO/N²` when it
    agrees within 0.5 EV with the measured median ratio of luminance over pixels
    whose pre-balance level is 0.01–0.9 and unclipped in both (at least 64
    samples); otherwise the measured ratio (`exposure_source` `measured`). With
    no measurement the EXIF value is used (`exif-unverified`); with neither the
    merge is refused.
  - Clipping: a pixel is clipped when the pre-balance raw level of it or a 3x3
    neighbour is at least 0.97.
  - Weights: `1 − (2l − 1)^12` of the pre-balance level `l`, zero when clipped,
    below 0.002 or outside the frame after the shift.
  - `deghost` `off` (default) \| `low` \| `medium` \| `high` rejects pixels of a
    non-reference input whose normalized luminance differs from the
    reference's by more than 1, 0.6 or 0.35 stops, dilated 3x3. The rejected
    count is recorded as `ghost_pixels`.
  - Where no input has weight, the darkest input (reference level ≥ 0.5) or
    the brightest (otherwise) that covers the pixel is used.
  - The result is the weighted mean of `rgb / factor`, in the reference's scale
    and multiplied by `2^BaselineExposure` of the reference.
- Panorama: inputs are given left to right; the middle input `(n-1)/2` is the
  reference.
  - `projection` `cylindrical` (default) \| `spherical` \| `perspective`. The
    focal length in output pixels is `--focal`, else EXIF focal length / 36 mm ×
    the long edge (a full-frame assumption, recorded as `focal_source`), else
    the long edge.
  - Features: Harris corners (central gradients, 5x5 window, k = 0.04, 4-pixel
    non-maximum suppression, at most 500) on the natural log of luminance of a
    proxy whose long edge is at most 800, described by normalized 9x9 patches.
    Matches are mutual best normalized cross-correlations of at least 0.8 that
    pass the ratio test `1 − s1 ≤ 0.6 (1 − s2)`.
  - Adjacent pairs are fitted by RANSAC with a splitmix64 generator seeded by
    `--seed` (default 0, shared by all pairs in order) and a 2 proxy-pixel
    threshold: a translation of projected coordinates for cylindrical and
    spherical (1000 iterations, refined by the inlier mean, at least 6 inliers),
    a homography of centered coordinates for perspective (1000 iterations of 4
    points, refined by least squares, at least 8 inliers). Too few inliers are
    refused with the pair named. Placements are chained and re-based on the
    reference. There is no exposure compensation between frames.
  - The canvas spans the projected borders (33 samples per edge); its size is
    checked against the output limit before rendering. Each canvas pixel is
    labeled with the input whose own nearest edge is farthest (ties to the
    lower index); unlabeled pixels are transparent.
  - Seams are blended with five-level Laplacian pyramids (normalized 5-tap
    filters, invalid source pixels filled by push-pull) weighted by the label
    masks' Gaussian pyramids, one input at a time in a box with a 64-pixel
    margin. Bilinear sampling. No boundary warp fills the frame.
- Output: a DNG written by pentool (`src/photo/dngout.rs`): LinearRaw with 3
  samples in `f16`, deflate with predictor 34894, `ColorMatrix1` describing the
  working primaries and a neutral `AsShotNeutral` of 1,1,1. Values are scaled
  by `2^-k` for the smallest integer `k` (at most 10) that keeps every sample at
  or below 0.9, and `k` is stored as `BaselineExposure`. Transparency is an
  8-bit transparency mask (`NewSubfileType` 4) in a SubIFD, which development
  warps with the image into alpha. The output is limited to 120 megapixels,
  32768 per side, and what a derived DNG can develop within the 2 GiB develop
  budget (about 41 megapixels). A larger HDR is refused before decoding and a
  larger panorama after alignment, before rendering; both errors suggest a
  `--scale`. An embedded output is limited to 128 MiB; `--external PATH`
  stores it at a new document-relative path.
- Attribution: the asset's `derived` holds `operation`, `algorithm` (1),
  `inputs` (digests, in order), `settings` (the requested options) and
  `alignment` (HDR: reference and per-input `shift`, `exposure`,
  `exposure_source`, `ghost_pixels`; panorama: projection, reference, seed,
  focal, canvas, per-input `translate` or row-major `homography`, and per-pair
  match counts). Its other facts are verified against its bytes like a raw
  source's. The same inputs and options produce the same bytes, so a repeat
  merge reuses the asset.
- The new photo's master variant has import defaults without stage-2 noise
  reduction, which the inputs already received. `--settings first` (default
  `import`) also copies the first input's master `tone`, `presence`, `curves`,
  `hsl`, `grading`, `monochrome`, `effects` and `detail.sharpening`, and for HDR
  its `crop`. It never copies `raw`, `white_balance`, `calibration` or stage-2
  detail (already applied to the inputs), `lens` (a derived DNG has no capture
  facts), `geometry` or `local` (framed on one input), or a panorama's `crop`.
- Cancellation or failure leaves no derived asset, no photo entry and no
  external file: the merge runs in memory, and the document and external DNG
  are committed together, the DNG first through a temporary file and rename,
  and removed again if the document commit fails. An existing `--external`
  file is refused.

### Wide-gamut and HDR delivery

Stage 11 (`src/photo/output.rs`) turns the developed working image into output
codes. In order, per pixel:

1. **Look and tone.** Scene-referred sources apply the camera profile's look
   table (`ProfileLookTableDims`/`Data`/`Encoding`, 50981/50982/51108, read and
   validated like the hue/sat map) and then its `ProfileToneCurve` (50940).
   The tone curve holds 2–4096 points from (0, 0) to (1, 1) with increasing x
   and non-decreasing y in 0–1; a curve of only identity points counts as
   absent. It is interpolated by a monotone cubic (Fritsch–Carlson tangents)
   and extended linearly with its end slopes outside [0, 1]. It is applied
   hue-preservingly like the DNG SDK's `RefBaselineRGBTone`: the curve maps
   the largest and smallest channel, and the middle channel keeps its relative
   position between them. `matrix-only` drops both. Derived merge DNGs carry
   neither.
2. **Gamut statistics.** For each named space, a pixel with working luminance
   `Y > 0` counts as outside that gamut when any of its linear channels there
   is below `-1e-4 · Y`.
3. **Output matrix** to linear output RGB `v`, relative to SDR white.
4. **Shoulder.** Scene-referred sources apply the process-1 shoulder on the
   max(R,G,B) norm, scaled to the output peak `P` (1 for SDR,
   `2^headroom` for HDR): with `r = m / P`, `f(m) = m` for `r <= 0.8`, otherwise
   `P · (0.8 + 0.2 (r - 0.8) / (r - 0.6))`. Every channel is scaled by `f(m)/m`,
   so channel ratios (hue) are kept, the slope is 1 at the knee, and the
   asymptote is `P`.
5. **Gamut mapping.** A pixel is out of gamut when any channel's signal
   (`v` times the signal scale below) lies outside the range that quantizes
   to a code (the clip report range). `relative-colorimetric` clips each
   channel. `perceptual` (the default) moves the pixel toward the neutral of
   the same output luminance `Y` (a straight line in chromaticity toward the
   white, so the dominant wavelength is kept) just far enough that every
   channel lies in `[0, P]`: `s = min(Y / (Y - c))` over channels `c < 0` and
   `(P - Y) / (c - Y)` over channels `c > P`, then `v' = Y + s (v - Y)`. When
   `Y <= 0` or `Y >= P`, only clipping can help and the pixel is clipped. Only
   out-of-gamut pixels are touched, so in-gamut values round-trip exactly.
6. **Quantization** with the output transfer, as in "Working representation".

Display-referred sources skip steps 1 and 4.

**SDR output** is any named space, optionally `:linear`, at 8 or 16 bits
(default `srgb`, 8). The PNG carries an `iCCP` chunk holding a generated ICC
v4.3 matrix/TRC display profile (`src/photo/icc.rs`: fixed date 2026-01-01,
zero profile ID, D50 PCS, `desc`, `cprt`, `wtpt`, `chad`, `rXYZ`/`gXYZ`/`bXYZ`
from the Bradford-adapted primaries, and one shared `para` curve), so the same
space always produces the same bytes. The header rendering intent is 0 for
`perceptual` and 1 for `relative-colorimetric`. Spaces with ITU-T H.273 codes
also carry `cICP` (primaries, transfer, matrix 0, full range): `srgb` (1, 13),
`display-p3` (12, 13), `rec2020` (9, 1), with transfer 8 for `:linear`.
`adobe-rgb-1998` and `prophoto` have no `cICP`.

**HDR output** (`--hdr pq|hlg`) is `rec2020` at 16 bits; another space or 8 bits
is `[invalid-input]`. `--headroom` is 0–4 stops above SDR white (default 1.5);
it is refused without `--hdr`. SDR white is 203 cd/m² (BT.2408):

- `pq`: the linear signal is `v · 203 / 10000`; the mastering peak is
  `203 · 2^headroom` cd/m².
- `hlg`: the linear scene signal is `v · E(0.75)`, so SDR white lands at signal
  0.75, and the headroom is at most `-log2 E(0.75)` ≈ 1.92 stops; more is
  `[invalid-input]`. Light levels assume the nominal 1000 cd/m² display:
  `nits = 1000 · Ys^0.2 · E` with `Ys = 0.2627 R + 0.6780 G + 0.0593 B`.

HDR PNGs carry, in order after `IHDR`: `cICP` (9, 16 or 18, 0, 1); `mDCV` with the
Rec. 2020 primaries and D65 white in 0.00002 units, a maximum luminance of
`ceil(max(peak, MaxCLL) · 10000)` (peak 1000 cd/m² for HLG) and a minimum of 1
(0.0001 cd/m²); and `cLLI` with the measured MaxCLL and MaxFALL. HDR output has
no ICC profile. The SDR rendition is the same command without `--hdr`. Gain-map
JPEG (ISO 21496-1) is excluded from v0.11.0.

**Measurement and validation.** Statistics are computed from the output codes,
not the floating-point image. Pixels whose alpha code is 0 are skipped and
counted as `transparent_pixels`. The histogram has `bins` equal code ranges per
channel (bin = `code · bins / (max + 1)`, 64 by default, 1–1024). The maximum
luminance is relative to SDR white. For HDR, MaxCLL is the brightest pixel's
largest channel in cd/m² and MaxFALL the frame average of that value
(CTA-861.3). Before a file is accepted, the written PNG is decoded again: its
codes must equal the rendered codes, and its `cICP`, `mDCV`, `cLLI` and `iCCP`
chunks must equal those recomputed from the decoded codes, with
`MaxFALL <= MaxCLL <= mDCV maximum`. A mismatch is `[malformed-resource]` and
nothing is written.

The editor's Display P3 preview with matching `iCCP`/`cICP`, and its clipping
and gamut overlays, belong to the editor work (item 15). The browser and
operating system own monitor calibration. HDR display preview is out of scope:
the editor shows the SDR rendition.

## Catalog (`photography`)

```json
"photography": {
  "engine": 1,
  "working_space": "prophoto-linear",
  "assets":      {"sha256:...": {"...": "asset records"}},
  "profiles":    {"sha256:...": {"kind": "camera|lens|icc", "...": "..."}},
  "photos":      [{"...": "photo entries, in import order"}],
  "stacks":      {"burst-1": {"photos": ["a", "b", "c"]}},
  "collections": {"picks": {"kind": "smart", "query": "pick:pick"}},
  "recipes":     {"client-proof": {"...": "recipe"}}
}
```

Profiles use the same storage contract as assets (`storage`, `byte_length`).

### Photo entries, variants and snapshots

```json
{"id": "hero", "source": "sha256:...", "name": "capture",
 "rating": 4, "pick": "pick", "label": "green",
 "keywords": ["harbor", "dusk"], "title": "", "caption": "",
 "creator": "", "copyright": "", "location": {"city": "", "country": ""},
 "variants": [{"id": "master", "name": "Master", "develop": {"process": 1}},
              {"id": "warm-editorial", "name": "Warm editorial", "develop": {"process": 1}}],
 "snapshots": [{"id": "before-grade", "name": "Before grade", "variant": "master", "develop": {"process": 1}}]}
```

- IDs for photos, variants, snapshots, stacks, collections and recipes match
  `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`. Photo IDs are unique in the document; variant
  and snapshot IDs are unique within their photo. `variants[0]` is always `master`.
- A variant is a complete `develop` object, not a patch. Variants and snapshots
  reference the photo's source and never duplicate bytes. Brush-mask tiles are
  shared by digest.
- A snapshot is an immutable copy of a variant's settings. `photo snapshot
  restore` copies it back into its variant (one undoable transaction).
- Organization fields belong to the photo, not the variant: `rating` 0–5, `pick`
  `none`\|`pick`\|`reject`, `label` `none`\|`red`\|`yellow`\|`green`\|`blue`\|`purple`,
  and `keywords` (at most 64, each 1–64 characters, NFC normalized, unique ignoring
  case). Absent means 0, `none`, `none`, `[]`.
- Limits: 10,000 photos, 64 variants and 64 snapshots per photo, 32 local
  adjustments per variant, 256 profiles, 1,024 stacks, 256 collections (a manual one
  holds at most 10,000 IDs), 64 recipes.

### Synchronized settings

`photo settings sync DOC SOURCE[/VARIANT] --to TARGETS [--groups G,...] [--except
G,...] [--auto-per-photo]` copies whole settings groups from one variant to target
variants (default `master`). `TARGETS` is a selection query, `@ids.json` (a JSON
array of `"photo[/variant]"` strings), or a comma-separated list of
`photo[/variant]` entries. Groups are the `develop` group names except `process`,
which is fixed by the engine; the default is every group. `local` copies masks with
normalized coordinates (brush tiles stay shared by digest), and `crop` and
`geometry` can be excluded. A group the source does not set is removed from the
target. At most 10,000 targets; the source itself and repeated targets are
refused.

- Explicit values copy as values. Mode values such as `white_balance.mode:
  "as-shot"` copy as modes, so each target keeps its own as-shot balance.
- An `auto`-resolved value copies as its resolved value, with its provenance. With
  `--auto-per-photo`, the analysis runs again for each target and stores each
  target's own result: gray-world white balance (`white_balance.auto`), upright
  (`geometry.auto`) and auto tone (`tone.auto`), in that order.
- `presence.dehaze_airlight` is a measurement of the image, so it is measured again
  on every raw target where dehaze is active and the airlight inputs, `presence`
  or `local` changed.
- `raw` and `white_balance` are skipped for a target whose source kind differs, and
  `raw` is skipped when it names a stored camera profile for another camera (unless
  the profile was added with `--force-model`). Analyses of a rendered target are
  skipped and reported. Each skipped item is reported per target; nothing is skipped
  silently.
- The whole sync is one transaction. Its result (and dry run) reports, per target,
  `{photo, variant, changed: [groups], skipped: [{group, reason}], resolved}`.

Variants: `photo variant add DOC PHOTO ID [--from VARIANT | --from-snapshot SNAP]
[--name TEXT]` copies a complete develop object; `photo variant rename DOC PHOTO ID
[--name TEXT]` sets or clears the display name; `photo variant remove DOC PHOTO ID`
refuses `master`, a variant with snapshots and a variant shown by a photo node.
Snapshots: `photo snapshot add DOC PHOTO[/VARIANT] ID [--name TEXT]`, `photo
snapshot restore DOC PHOTO ID` and `photo snapshot remove DOC PHOTO ID`. New IDs
that already exist are `[conflict]`.

### Stacks, collections and search

- A photo belongs to at most one stack. A stack's first photo is its top.
- `photo search DOC QUERY [--limit N --offset N]` combines terms with AND:
  `rating>=N` (also `=`, `<=`), `pick:pick|reject|none`, `label:COLOR`,
  `keyword:TEXT`, `camera:TEXT`, `lens:TEXT`, `iso>=N`, `focal>=N`,
  `captured>=YYYY-MM-DD`, `captured<=YYYY-MM-DD`, `stack:top`, `has:variants`,
  `has:local`, `id:GLOB`, `collection:NAME`. Matching is case-insensitive.
  Results are ordered by catalog order. Output follows the tree/search contract:
  `matches`, `returned`, `offset`, `limit`, `has_more`, with compact rows (ID, source
  kind, dimensions, rating, pick, label, variant count) and no settings. Search uses
  only document JSON and never decodes sources.
- A collection is either `{"kind": "manual", "photos": [...]}` or
  `{"kind": "smart", "query": "..."}`.
- `photo contact-sheet DOC --selection Q --columns N [--cell PX] --out FILE.png|.pdf`
  renders cached or freshly developed thumbnails with captions built from
  `{id}`, `{name}`, `{rating}` and `{variant}`.

### Searchable capture summary

`assets[digest].capture` holds only `make`, `model`, `lens`, `focal_length` (mm),
`aperture` (f-number), `exposure_time` ([numerator, denominator]), `iso`, `flash`
(bool) and `captured` (`DateTimeOriginal` as local ISO 8601, with `OffsetTimeOriginal`
when present). GPS, serial numbers, owner names and maker notes are never copied
into document JSON. They exist only inside the source bytes (see
[privacy](#metadata-privacy)).

## Metadata privacy

Metadata is grouped into categories:

| Category | Fields |
|---|---|
| `copyright` | copyright notice, rights usage terms |
| `creator` | creator, creator job title and contact |
| `description` | title, caption, headline |
| `keywords` | keywords |
| `location` | IPTC city, state, country, sublocation |
| `camera` | make, model, lens model, focal length, aperture, exposure, ISO, flash |
| `timestamps` | DateTimeOriginal, CreateDate, ModifyDate, sub-seconds, offsets |
| `gps` | the whole GPS IFD and XMP `exif:GPS*` |
| `serials` | body and lens serial numbers, ImageUniqueID |
| `identity` | camera owner name, EXIF Artist when not chosen as creator, person and face regions |
| `software` | `pentool <version>`, written only when kept |

Values for `copyright`, `creator`, `description`, `keywords` and `location` come
from the photo entry. The other categories come from the source bytes. Maker notes,
embedded previews and thumbnails are never exported.

Export policy: `metadata: {"policy": P, "include": [...], "exclude": [...]}`.
Policies are `none` (the default, which matches today's image exports), `copyright`
(copyright and creator), `public` (everything except `gps`, `serials`, `identity`
and `timestamps`) and `all-including-private`. `include` and `exclude` adjust the
policy by category. Writing `gps`, `serials` or `identity` requires either that
explicit policy name or an explicit `include` of the category. Writers: JPEG uses
EXIF and XMP APP1 segments (no IPTC-IIM); PNG uses `eXIf` and iTXt
`XML:com.adobe.xmp`; TIFF uses the EXIF IFD and tag 700. Serialization is
deterministic, with no generated timestamps or UUIDs.

`photo metadata DOC ID` shows metadata grouped by category, with `gps`, `serials`
and `identity` redacted unless `--reveal CATEGORY` is given. `photo privacy-report
DOC` lists sources whose bytes contain private categories. `package` prints the same
warning, because packaging carries the source bytes.

## Output recipes and batch delivery

```json
{"format": "jpeg", "quality": 85, "chroma": "420", "bit_depth": 8,
 "color_space": "srgb", "intent": "perceptual",
 "resize": {"mode": "long-edge", "value": 2048, "enlarge": false}, "ppi": 72,
 "sharpen": {"target": "screen", "amount": "standard"},
 "naming": "{photo}-{variant}", "metadata": {"policy": "copyright"}}
```

- `format` `jpeg` (8-bit, quality 1–100, chroma `420`\|`444`), `png` (8 or 16
  bit, or HDR), or `tiff` (8 or 16 bit, `none`\|`deflate`). WebP, AVIF, HEIC, JPEG
  XL and DNG export are out of scope for v0.11.0.
- `resize.mode` `none`\|`long-edge`\|`short-edge`\|`width`\|`height`\|`megapixels`\|
  `percent`\|`print`. `print` takes `{width, height, unit: in|cm, ppi, fit:
  "error"|"crop"|"pad"}`. An aspect mismatch is an error unless `fit` says
  otherwise; pentool never crops silently. Resampling is separable Lanczos-3 in
  linear light.
- `sharpen.target` `screen`\|`matte`\|`glossy`, `amount` `low`\|`standard`\|`high`,
  applied after resize.
- `naming` tokens: `{photo}`, `{variant}`, `{name}`, `{recipe}`, `{rating}`,
  `{seq:N}` (1–6 digits) and `{captured:YYYYMMDD}`, which is allowed only when
  `timestamps` is exported, so a filename cannot leak what the metadata strips.
  Names are sanitized to `[A-Za-z0-9._-]`.
- Built-in recipes, which a document recipe may not shadow:
  - `web-gallery`: sRGB JPEG, quality 85, long edge 2048, screen sharpening,
    `copyright` metadata.
  - `social`: sRGB JPEG, quality 90, long edge 1080, screen sharpening, `none`.
  - `archive-master`: 16-bit `prophoto` TIFF with deflate, no resize, no
    sharpening, `public` metadata.
  - `photo-lab`: sRGB JPEG, quality 95, 300 ppi, `print` resize (size given per
    run), glossy sharpening, `copyright` metadata.

`photo export DOC --selection Q|picks|@ids.json --recipe R --out DIR [--variant V |
--all-variants] [--overwrite] [--dry-run]` works in four steps:

1. Plan first: resolve the photos, compute every output name, size and the
   estimated bytes, and detect collisions (an error unless `--overwrite`). At most
   10,000 outputs per run.
2. Render into `DIR/.pentool-export-<run>/`, one file at a time, with bounded memory
   (one develop at a time per worker, `MAX_DEVELOP_BYTES` each).
3. Only after every output succeeds, rename each into `DIR`. On failure or
   cancellation, the staging directory is deleted and nothing reaches `DIR`.
4. Report the outputs (`{photo, variant, path, width, height, bytes, sha256}`) as
   JSON. Export never modifies the document.

`--dry-run` prints the plan. It uses the same planner as a real export.

## Caches

Rendered previews and thumbnails are disposable. They are never part of a `.pen`
or a package.

- Location: `.pentool/cache/photo/` next to the document, beside the existing hidden
  history.
- Key: SHA-256 of canonical JSON holding `{engine, process, source digest, profile
  digests, develop, mask digests, output space, size}`. The key is also stored
  inside the cached PNG, and an entry is used only when the two match.
- Bound: 2 GiB by default, set with `PENTOOL_PHOTO_CACHE_BYTES`, and evicted
  least-recently-used. `photo cache clear DOC` empties it, and `--no-cache` bypasses
  it.
- A cache write failure never fails the command, and no read-only command modifies
  the document.

## Page placement (`photo` node)

```json
{"kind": "photo", "id": "hero-on-cover", "photo": "hero", "variant": "master",
 "x": 0, "y": 0, "width": 1200, "height": 800, "fit": "cover", "position": [0.5, 0.5],
 "opacity": 1, "blend_mode": "normal"}
```

- A photo node takes the v6 base properties (compositing, masks, clipping,
  effects, transforms) and the image node's `fit` and `position`. Cropping belongs
  to the variant.
- The developed variant is converted to `srgb8` at the node boundary, using the
  output rendering in stage 11 and the `perceptual` intent. Page export to PNG, SVG
  and PDF embeds that 8-bit rendition. Wide-gamut and 16-bit delivery go through
  recipes.
- Tree and search report `kind: "photo"` with `photo`, `variant`, bounds and layer
  state, and never settings. A reference to a missing photo or variant is
  `[missing-resource]`.
- Until a build includes the development pipeline, rendering a page that holds a
  photo node fails with `[unsupported-capability]` instead of drawing a placeholder.

## Determinism

Engine 1 uses `f32` and `f64` arithmetic with `+ - * /` and `sqrt` only. It uses
no fused multiply-add, no platform `libm` functions, and no reductions whose
ordering depends on the thread count. Transcendental functions (`log2`, `exp2`,
`pow`, `sin`, `cos`, `atan2`) are in-tree, fixed-coefficient implementations shared
with the raster engine's approach. Parallelism is per tile with independent outputs.
Seeds (grain, RANSAC) are stored in the document. Golden tests compare SHA-256
hashes of 16-bit outputs on Linux, Windows, macOS ARM and macOS Intel.

## Commands (CLI surface)

```sh
pentool raw add catalog.pen hero --file ./capture.dng --external --camera-profile auto
pentool raw info catalog.pen hero
pentool raw develop catalog.pen hero --exposure 0.7 --temperature 5400 --dry-run
pentool photo import catalog.pen --file ./scan-16bit.tif --input-profile adobe-rgb-1998
pentool photo variant add catalog.pen hero warm-editorial [--from master | --from-snapshot before-grade]
pentool photo snapshot add catalog.pen hero/master before-grade
pentool photo local add catalog.pen hero/master sky --linear 0.5,0,0.5,0.45 --exposure -0.4
pentool photo snapshot restore catalog.pen hero before-grade
pentool photo settings sync catalog.pen hero --to @selected.json --except crop [--auto-per-photo]
pentool photo rate catalog.pen hero --rating 4 --pick pick --label green
pentool photo search catalog.pen "rating>=3 pick:pick" --limit 50
pentool photo merge-hdr catalog.pen bracket-1 bracket-2 bracket-3 --id hero-hdr [--deghost medium] [--reference 2] [--scale 0.5] [--settings first] [--external merged/hero-hdr.dng]
pentool photo merge-pano catalog.pen pano-1 pano-2 pano-3 --id harbor-pano --projection cylindrical [--seed 7] [--focal 2400]
pentool photo export catalog.pen --selection picks --recipe web-gallery --out ./delivery
pentool photo place catalog.pen hero --variant warm-editorial --layer layer-1 --width 1200 --height 800
```

`raw add` embeds the source by default (`--embed`); `--external` stores a path
relative to the document, and the file must stay inside the document's folder.
`--camera-profile` takes `auto`, `embedded`, `matrix-only` or the `sha256:` digest
of a profile already in `photography.profiles`. A document older than v7 is upgraded
explicitly, and identical bytes are stored once. The result is
`{photo, asset, deduplicated, upgraded, variant, unsupported_opcodes}`. `raw info`
prints the photo, its storage, the recorded source facts, the variants and the
snapshot count.

`photo profile add DOC --file PROFILE.dcp [--force-model]` verifies a DNG camera
profile and stores it, embedded, in `photography.profiles` (identical bytes are
stored once; `--force-model` on a stored profile records the override). It
upgrades a document older than v7. The result is `{profile, name,
unique_camera_model, embed_policy, force_model, calibrations, hue_sat_map,
deduplicated, upgraded}`.

`raw develop DOC PHOTO [--variant ID] [--camera-profile auto|embedded|matrix-only|DIGEST]`
takes at most one of `--as-shot`, `--temperature K [--tint T]`, `--neutral r,g,b`,
`--sample x,y,radius` and `--suggest`. It stores the setting in the variant
(default `master`).

It also takes:

- `--lens-profile none|embedded-opcodes|DIGEST`;
- `--set KEY=JSON` and `--unset KEY` (repeatable), for dotted develop keys of
  1–4 lowercase parts such as `lens.distortion` or `crop.rect`. `process` is
  fixed. Unsetting a key that is not set is `[invalid-input]`, and emptied
  groups are removed;
- `--upright off|level|vertical|full|guided`, with `--guide x1,y1,x2,y2`
  (repeatable, at most 4; guided only);
- `--exposure EV`, a shorthand for `--set tone.exposure=EV`;
- `--auto-tone`, which resolves the tone values. It cannot be combined with
  `--exposure` (`[invalid-input]`).

The changes are applied in this order:

1. unset;
2. set;
3. camera profile;
4. lens profile;
5. guides;
6. white balance;
7. upright;
8. auto tone;
9. dehaze airlight.

The result is validated as a whole before the document is written. Each
analysis sees every change before it.

The report is `{photo, variant, camera_profile, lens_profile, white_balance,
monochrome, resolved: {xy, temperature, tint, neutral}, frame, upright?, tone?,
dehaze_airlight?}`:

- `frame` is the `photo info` frame;
- `upright` holds the solved keys and the segment count;
- `tone` and `dehaze_airlight` appear when auto tone ran or the airlight was
  resolved.

`photo profile add DOC --lens FILE.json` stores a pentool lens profile. The result
is `{profile, kind, name, imported_from, samples, unsupported, deduplicated,
upgraded}`. `photo profile import-lcp DOC FILE.lcp` converts and stores an LCP and
reports the same fields, with `unsupported` listing what was not converted.

`photo info DOC PHOTO [--variant ID]` resolves a variant's frame without decoding
pixels: `{decoded, oriented, output, orientation, crop: {constrained, rect},
lens: [steps], geometry, invalid_pixels}`.

`photo render DOC PHOTO [--variant ID] --out FILE.png [--space NAME] [--depth 8|16]
[--intent perceptual|relative-colorimetric] [--hdr pq|hlg] [--headroom STOPS]`
develops the variant through stage 11 and writes a verified, tagged PNG through
a temporary file and rename. It reports the frame plus the white,
`invalid_pixels`, `space`, `depth`, `clipped_pixels`, `bytes` and `delivery`:
`{space, transfer, depth, intent, hdr: {transfer, headroom, reference_white_nits,
peak_nits} | null, cicp, icc, pixels, transparent_pixels, out_of_gamut_pixels,
mapped_pixels, clipped_pixels, outside_gamut: {space: count}, max_luminance,
histogram: {bins, r, g, b}}`, plus `max_cll` and `max_fall` for HDR. Only raw
sources render for now; a rendered source is `[unsupported-capability]`.

`photo inspect DOC PHOTO [--variant ID] [the same output options] [--bins N]`
runs the same development and stage 11 without writing a file and reports
`{photo, variant, width, height, delivery}`.

`raw develop` is a shorthand for setting values in `develop` groups. Every mutating
command takes `--dry-run` and `--if-revision`, commits through the shared
transaction, and creates one history entry. `POST /api/photo` (stateless, like
`POST /api/raster`) shares the CLI's dispatcher, and so does
`photo batch DOC OPS.json`, which takes 1–256 operations.

## Error codes

These reuse the stable codes (`unsupported-capability`, `limit-exceeded`,
`malformed-resource`, `hash-mismatch`, `missing-resource`, `unsafe-path`,
`locked-node`, `cancelled`) and add one code, `invalid-develop`, for settings that
are out of range, unknown, or inapplicable.

## Explicitly out of scope for v0.11.0

The following are out of scope:

- non-DNG RAW containers, X-Trans and other non-Bayer CFAs, and lossy or JPEG XL
  DNG;
- LUT-based ICC input profiles, CMYK/Lab, soft proofing, and print workflows, all of
  which belong to v0.12.0;
- gain-map JPEG and HDR display preview;
- WebP, AVIF, HEIC, JPEG XL and DNG export;
- boundary-warp panorama fill;
- face and person recognition;
- tethering;
- cloud sync;
- any runtime network access.

## Conformance fixtures

`docs/fixtures/photo-conformance.json` lists each fixture with its SHA-256, the
expected outcome (`ok` or an error code), and the roadmap item that first exercises
it. The current set:

- `photo-dng-rggb16.dng`: valid 16x16 RGGB, 12-bit values in 16-bit uncompressed
  strips, default crop 12x12, embedded profile, and EXIF/GPS/serial/owner fields
  for privacy tests (all synthetic);
- `photo-dng-linear16.dng`: valid 8x8 three-sample LinearRaw, 16-bit;
- `photo-dng-ifd-loop.dng`: the IFD chain points back to IFD0
  (`malformed-resource`);
- `photo-dng-strip-out-of-bounds.dng`: a strip outside the file
  (`malformed-resource`);
- `photo-dng-oversize.dng`: declares 40000x40000 (`limit-exceeded`, refused before
  allocation);
- `photo-dng-jpegxl.dng`: compression 52546 (`unsupported-capability`);
- `photo-dng-xtrans.dng`: a 6x6 CFA (`unsupported-capability`);
- `photo-raw-cr2-header.cr2`: a CR2 signature (`unsupported-capability`, convert
  to DNG);
- `photo-v7.pen`: a valid catalog with two photos, variants, a snapshot, a local
  mask, a stack, a collection, a recipe, and a photo node;
- `photo-v7-invalid-variant.pen`: a node referencing a missing variant
  (`missing-resource`);
- `photo-v7-invalid-develop.pen`: an exposure of 9 EV (`invalid-develop`).

The manifest records each fixture's roadmap item, the item that first implements
its expected outcome. Every file is pinned byte for byte, and `.gitattributes`
disables line-ending conversion for them. `tests/v0110_photo_spec.rs` checks four
things:

- every digest;
- that every schema `$ref` resolves;
- that the fixtures agree with the schema's version and limits;
- that the current build refuses each v7 fixture with `unsupported-capability`
  without modifying it.

Item 3 replaces that last check with real validation.
