# v0.11.0 — Professional photography and RAW development

Build a Lightroom-class, non-destructive photography workflow on v0.7.0–v0.10.0.
This milestone owns RAW development, high-bit-depth RGB processing, lens correction,
local adjustments, variants, organization, and batch delivery. Commercial-print
color management moves to v0.12.0 so photography can ship as a coherent product.

## Product contract

- RAW and imported photographs remain immutable, content-addressed sources.
- Development settings, masks, crops, variants, and output recipes remain editable.
- Decode, demosaic, adjustment, histogram, and export paths are bounded Rust code.
- Existing sRGB documents retain their appearance unless explicitly converted.
- Projects render offline; camera databases and AI providers are not runtime needs.
- Metadata retention, especially GPS and identity data, is explicit.

v0.11.0 includes the photographic color foundation: declared input profiles,
scene-linear high-precision processing, wide-gamut RGB spaces, calibrated RGB
preview where supported, and explicit RGB export conversion. CMYK/Lab, spot inks,
separations, paper simulation, and PDF/X belong to v0.12.0.

## Must ship, in order

- [ ] **1. Freeze the photography specification.** Define RAW support, decode
  limits, demosaic order, working RGB spaces, precision, metadata, development
  stages, variants, caches, schemas, migrations, and conformance fixtures.
  *Status: drafted for review.* This item has four parts:
  - [`docs/photography-v1.md`](../../photography-v1.md), the specification;
  - [`docs/pen-format-v7.schema.json`](../../pen-format-v7.schema.json), the schema;
  - `docs/fixtures/photo-*` with the `photo-conformance.json` manifest;
  - `tests/v0110_photo_spec.rs`.

  The checkbox stays open until review and hosted CI pass.
- [ ] **2. Add high-bit-depth RGB paths.** Support 16-bit storage and a documented
  floating-point working representation without accidental 8-bit/sRGB round trips.
  *Status: implemented for review.* This item has four parts:
  - `src/photo/` (`math`, `color`, `pixels`, `png`);
  - the "Working representation" section of the specification;
  - the `photo-rgb16-p3.png` fixture;
  - `tests/v0110_photo_precision.rs`.

  Nothing user-facing calls this path until item 3. The checkbox stays open
  until review and hosted CI pass.
- [ ] **3. Implement bounded RAW ingestion.** Validate and decode a tested subset
  of RAW containers, retain source bytes, and report unsupported cameras clearly.
  Decision (item 1): the tested subset is DNG 1.0–1.7, read by pentool's own
  bounded decoder with no new dependency. Other RAW containers are recognized by
  signature and refused, and the error says to convert them to DNG.
  *Status: implemented for review.* This item has four parts:
  - the bounded reader in `src/photo/` (`tiff`, `ljpeg`, `dng`, `opcode`, `raw`);
  - the v7 catalog and photo-node validation in `src/photo/catalog.rs`;
  - `pentool raw add` and `pentool raw info`, with explicit v7 migration that
    never downgrades photo content;
  - `tests/v0110_raw_ingest.rs` and the item-3 fixtures in
    `tests/v0110_photo_spec.rs`.

  Photo nodes validate and appear in tree output, but rendering them is refused
  with `[unsupported-capability]` until the development pipeline (items 4–6)
  exists. The checkbox stays open until review and hosted CI pass.
- [ ] **4. Add camera profiles and white balance.** Support embedded and verified
  profiles plus as-shot, temperature/tint, sampled-neutral, and suggested balance.
- [ ] **5. Add lens and geometry correction.** Cover distortion, vignetting,
  chromatic aberration, perspective, leveling, rotation, and constrained crop.
- [ ] **6. Build the development stack.** Add exposure, highlights/shadows,
  whites/blacks, curves, clarity, texture, dehaze, vibrance, HSL, grading,
  monochrome, grain, vignette, calibration, and stable processing order.
- [ ] **7. Add detail processing.** Implement luminance/color noise reduction,
  sharpening, moiré reduction, defective-pixel handling, and defringe.
- [ ] **8. Add local adjustments.** Reuse gradient, radial, range, depth, and
  painted masks. AI-derived masks must first become materialized editable masks.
- [ ] **9. Add HDR and panorama merging.** Provide bounded alignment, deghosting,
  projection, seam blending, attribution, cancellation, and non-destructive output.
- [ ] **10. Add wide-gamut and HDR RGB delivery.** Define primaries, transfer
  functions, inspection, tone mapping, SDR conversion, metadata, and validation.
- [ ] **11. Add variants, snapshots, and synchronized edits.** Virtual copies do
  not duplicate sources; synchronization supports selected settings and exceptions.
- [ ] **12. Add local organization.** Provide ratings, picks, labels, keywords,
  stacks, contact sheets, compare/survey, and search without a required database.
- [ ] **13. Add metadata privacy.** Support a deliberate EXIF/IPTC/XMP subset and
  export policies for copyright, keywords, GPS, identity, serials, and timestamps.
- [ ] **14. Add output recipes and batch delivery.** Cover web, social, archive,
  and photo-lab dimensions, RGB profiles, precision, codec, naming, and sharpening.
- [ ] **15. Build photographer UX.** Add filmstrip/grid, loupe, culling, histogram,
  clipping warnings, development panels, masks, crop, compare, and copy/sync.
- [ ] **16. Prove conformance and performance.** Test hostile RAWs, profiles,
  precision, corrections, masks, merges, privacy, rollback, packaging, and caches.

## CLI direction

```sh
pentool raw add catalog.pen hero --file ./capture.dng --camera-profile auto
pentool raw develop catalog.pen hero --exposure 0.7 --temperature 5400 --dry-run
pentool photo variant add catalog.pen hero warm-editorial
pentool photo settings sync catalog.pen hero --to @selected.json --except crop
pentool photo merge-hdr catalog.pen bracket-1 bracket-2 bracket-3 --id hero-hdr
pentool photo export catalog.pen --selection picks --recipe web-gallery --out ./delivery
```

The complete command surface is in `docs/photography-v1.md`.

## Acceptance targets

- Develop, locally adjust, compare, and export variants from one immutable RAW.
- Round-trip a 16-bit wide-gamut fixture without undocumented precision loss.
- Cull and batch-process a substantial local shoot with bounded resources.
- Package sources, profiles, masks, variants, and recipes for offline reproduction.
- Cancel decode, merge, or export without partial mutation or orphan artifacts.

## Deferred to v0.12.0

Complete ICC workflows, CMYK/Lab, device-link profiles, ink limits, spot colors,
overprint, separations, paper simulation, print preflight, and PDF/X production.

## Acceptance demo

Import a RAW shoot, cull and rate it, correct camera/lens issues, develop at high
precision, merge brackets and panoramas, apply local masks, create variants, sync
selected settings, and export web, wide-gamut master, and photo-lab recipes. Move
the package offline and reproduce outputs without a catalog server or AI provider.
