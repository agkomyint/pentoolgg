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
  RGB, `v`. `v` is clipped to [0, 1]: a relative colorimetric clip, with perceptual
  gamut mapping arriving in item 10. NaN counts as 0. The code is the count of
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
  applied. Output PNGs are untagged until item 10 adds embedded profiles.
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
| 9 | Color: HSL, color grading, vibrance, saturation, monochrome mix | `hsl`, `grading`, `presence`, `monochrome` |
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
  and tint (xy through Robertson; tint uses the DNG SDK convention of ±1 tint unit =
  ±0.0003 in uv); a sampled `neutral` (the command samples a radius in camera RGB and
  stores the neutral); and `suggested` (auto: deterministic gray-world with
  clipped-pixel rejection on the analysis proxy, stored as temperature and tint with
  `auto: {"algorithm": "gray-world", "version": 1}`).

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
  - `range-luminance`: `min`, `max`, `smoothness` (working-space luminance after
    stage 6).
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

### HDR and panorama merges

`photo merge-hdr` (2–9 inputs) and `photo merge-pano` (2–64 inputs, at most 1.2
gigapixels of input in total) build a new derived source. They never modify the
inputs.

- Inputs are developed through stages 1–3 only (their `raw` and `white_balance`
  settings), so they are scene-linear in the working space.
- HDR: translation-only alignment with median-threshold bitmaps (integer,
  deterministic); exposure normalization from EXIF exposure time, aperture and ISO,
  verified against a measured ratio; a hat-weighted merge that rejects clipped and
  noise-floor values; `deghost` `off`\|`low`\|`medium`\|`high` against a chosen
  reference input.
- Panorama: `projection` `spherical`\|`cylindrical`\|`perspective`; Harris
  features, patch normalized cross-correlation, and RANSAC with a splitmix64 seed
  stored in the provenance; a bounded iteration count; and multiband (5-level)
  seam blending. Outside pixels are transparent. The panorama does not "boundary
  warp" to fill the frame.
- Output: a DNG written by pentool, LinearRaw with 3 samples in `f16`, deflate with
  predictor 34894, `ColorMatrix1` describing the working primaries and a neutral
  `AsShotNeutral` of 1,1,1. It is read back by the same decoder, so it develops
  like any raw. The output is limited to 120 megapixels. A larger result is refused
  before any work, and the error suggests `--scale`.
- Attribution: the asset's `derived` holds `operation`, `algorithm` version,
  `inputs` (digests, in order), `settings`, and the resolved alignment (per-input
  transforms, reference index, seed). A new photo entry, with a master variant at
  import defaults, points to the derived asset. `--settings first` copies the
  first input's master develop settings except `raw` and `geometry`.
- Cancellation or failure leaves no derived asset, no photo entry, no cache
  entry and no external file. External output is written to a temporary file in the
  document folder and renamed only when the transaction commits.

### Wide-gamut and HDR delivery

- Output spaces come from the color space table. SDR rendering of scene-linear
  data applies, in order: the profile look table and tone curve, if any; the
  process-1 shoulder on the max(R,G,B) norm, `f(m) = m` for `m <= 0.8` and
  `0.8 + 0.2 (m - 0.8) / (m - 0.6)` above (hue preserving, asymptote 1); and gamut
  mapping (`relative-colorimetric` clipping, or the default `perceptual`, which
  desaturates toward luminance along constant hue until the color is in gamut).
- HDR delivery is `rec2020` with `pq` or `hlg`, a `headroom` of 0–4 stops above SDR
  white (203 cd/m² reference white), written as 16-bit PNG with `cICP`, `mDCV` and
  `cLLI` chunks. An SDR rendition comes from the same pipeline. Gain-map JPEG
  (ISO 21496-1) is excluded from v0.11.0.
- `photo inspect` reports the histogram in the output space, clipped and
  out-of-gamut pixel counts, the maximum luminance, and the gamut used. Export
  validates its metadata against its pixels, for example `cLLI` against the
  measured maximum.
- The editor previews in sRGB by default, or in Display P3 when the browser offers
  a `display-p3` canvas. Preview images carry matching `iCCP`/`cICP`. The browser
  and operating system own monitor calibration. HDR display preview is out of scope:
  the editor shows the SDR rendition with clipping and gamut overlays.

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
variants (default `master`). `TARGETS` is a selection query, `@ids.json`, or a
comma-separated list of `photo[/variant]` entries. Groups are the `develop` group
names. `local` copies masks with normalized coordinates, and `crop` and `geometry`
can be excluded.

- Explicit values copy as values. Mode values such as `white_balance.mode:
  "as-shot"` copy as modes, so each target keeps its own as-shot balance.
- An `auto`-resolved value copies as its resolved value. With `--auto-per-photo`,
  the analysis runs again for each target and stores each target's own result.
- `raw` and `white_balance` are skipped for a target whose source kind differs.
  Each skipped item is reported per target; nothing is skipped silently.
- The whole sync is one transaction. Its dry run reports, per target, the groups
  that changed and the groups that were skipped.

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
pentool photo variant add catalog.pen hero warm-editorial [--from master]
pentool photo snapshot add catalog.pen hero/master before-grade
pentool photo local add catalog.pen hero/master sky --linear 0.5,0,0.5,0.45 --exposure -0.4
pentool photo settings sync catalog.pen hero --to @selected.json --except crop
pentool photo rate catalog.pen hero --rating 4 --pick pick --label green
pentool photo search catalog.pen "rating>=3 pick:pick" --limit 50
pentool photo merge-hdr catalog.pen bracket-1 bracket-2 bracket-3 --id hero-hdr
pentool photo merge-pano catalog.pen pano-1 pano-2 pano-3 --id harbor-pano --projection cylindrical
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
