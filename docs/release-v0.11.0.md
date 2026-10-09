# Pentool v0.11.0

Professional photography and RAW development. Pentool now develops DNG RAW files
non-destructively in a bounded, deterministic Rust engine, organizes and
synchronizes a shoot, and delivers it through output recipes, from the CLI or the
new Photos studio in the browser editor. The specification is
[`docs/photography-v1.md`](photography-v1.md); the roadmap is
[`docs/roadmap/v0.11.0/TASKS.md`](roadmap/v0.11.0/TASKS.md).

## Format

- `.pen` format version 7 adds a `photography` catalog (sources, photos, variants,
  snapshots, recipes, collections) and a `photo` page node. Schema:
  `docs/pen-format-v7.schema.json`.
- The first photo command upgrades a document to v7 in the same undoable
  transaction. `pentool migrate --target 7` upgrades v1–v6 losslessly; `--target 6`
  is refused while photo content exists. Older builds refuse v7 documents clearly
  and without modifying them.
- Existing sRGB documents keep their appearance.

## RAW and color

- Bounded DNG ingestion with pentool's own decoder: CFA (RGGB, BGGR, GRBG, GBRG)
  and LinearRaw, 8–16-bit integer and 16/24/32-bit float samples, uncompressed,
  lossless JPEG and deflate, strips or tiles, and opcode lists. Other RAW
  containers (CR2, NEF, ARW, X-Trans and so on) are refused with
  `[unsupported-capability]` and a suggestion to convert to DNG. Sources stay
  immutable and content-addressed, embedded or by verified relative path
  (`raw add`, `raw info`).
- 16-bit storage and an `f32` scene-linear working representation with no
  accidental 8-bit or sRGB round trip.
- Camera profiles (embedded, `.dcp`, dual-illuminant interpolation) and white
  balance by as-shot, temperature/tint, sampled neutral or suggestion
  (`photo profile add`, `raw develop`).
- Wide-gamut and HDR RGB delivery: sRGB, Display P3, Adobe RGB, Rec. 2020 and
  ProPhoto, 8 or 16 bits, perceptual or relative-colorimetric mapping, tagged PNG
  with ICC/cICP, HDR tone mapping and inspection (`photo render`, `photo inspect`).

## Development

- Lens and geometry: distortion, vignetting, chromatic aberration, perspective
  (upright), leveling, rotation and constrained crop, with lens profiles.
- The development stack: exposure, auto tone, highlights/shadows,
  whites/blacks, curves, clarity, texture, dehaze, vibrance/saturation, HSL,
  color grading, monochrome, grain and vignette.
- Detail: defective pixels, luminance and color noise reduction, moiré reduction,
  defringe and capture sharpening.
- Local adjustments with linear, radial, range and painted (`photo mask paint`)
  masks. Depth masks are refused until a depth source exists.
- HDR and panorama merges into derived, non-destructive DNG sources with
  alignment, deghosting, projection, seam blending, attribution and cancellation
  (`photo merge-hdr`, `photo merge-pano`, optional `--external`).

## Workflow and delivery

- Variants (virtual copies), snapshots and settings sync with included or excluded
  groups (`photo variant`, `photo snapshot`, `photo settings sync`).
- Local organization without a database: ratings, picks, labels, keywords,
  stacks, collections, a query language, contact sheets and compare
  (`photo search`, `photo rate`, `photo keyword`, `photo stack`,
  `photo collection`, `photo contact-sheet`, `photo compare`).
- Metadata privacy: a deliberate EXIF/IPTC/XMP subset and export policies for
  copyright, keywords, GPS, identity, serials and timestamps
  (`photo metadata`, `photo privacy-report`, `photo describe`, `--metadata`), and a
  privacy warning from `package pack`.
- Output recipes and batch delivery: built-in `web-gallery`, `social`,
  `archive-master` and `photo-lab`, plus document recipes; sizes, print
  dimensions, frames, RGB space, depth, PNG/JPEG/TIFF, naming and output
  sharpening. Exports are staged and leave nothing behind on failure or
  cancellation (`photo export`, `photo recipe`).
- The Photos studio in the browser editor: grid and filmstrip, loupe, compare,
  culling keys, histogram with clipping warnings, clipping/gamut/mask overlays,
  development panels, crop, radial/linear/brush masks, copy/sync, variants,
  snapshots and keywords. Every preview is a Rust render and every edit is a
  revision-guarded, undoable transaction.
- `photo` nodes are drawn on pages: page export (PNG, SVG, PDF) and the editor
  canvas show the developed variant as sRGB, placed with `fit` and `position`.
  Each render develops the photo at full size, so pages holding many large photos
  redraw slowly in the editor.
- A disposable preview cache in `.pentool/cache/photo/` (`photo preview`,
  `photo cache clear`, `serve --no-cache`, `PENTOOL_PHOTO_CACHE_BYTES`). It never
  enters packages and page exports never read it.

## Quality

- `tests/v0110_conformance.rs`: hostile DNG, profile, lens and PNG mutation
  sweeps; pinned develop and merge digests; rollback of about thirty failing
  commands; the cache contract; photo-node page rendering; and offline
  reproduction of every export from an installed package.
- Cancellation of decode, merge and export is tested at every checkpoint.
- Fixed before release: a hostile `.dcp` whose table sizes overflowed could panic;
  it is now refused with `[limit-exceeded]`.
- `pentool benchmark --photo` measures ingest, culling, development, previews and
  batch export. Measurements are in `docs/performance.md`.

## Building

`Cargo.toml` now declares `rust-version = "1.85"`, the oldest toolchain the
release is built and tested with. CI checks Clippy on current stable.

## Out of scope

CMYK/Lab, spot inks, separations, soft proofing for print and PDF/X are planned
for v0.12.0. HDR display preview in the editor is not included.
