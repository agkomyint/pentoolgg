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
  *Status: implemented for review.* This item has four parts:
  - the DNG SDK color kernels in `src/photo/profile.rs`: profile reading,
    interpolation, `NeutralToXY`, `SetWhiteXY`, hue/sat maps, Robertson
    temperature and tint, sampling and the gray-world suggestion;
  - `raw` and `white_balance` develop validation in `src/photo/develop.rs`, which
    the catalog applies to every variant and snapshot;
  - `pentool photo profile add` (`.dcp`, `--force-model`) and `pentool raw develop`
    (`--camera-profile`, `--as-shot`, `--temperature`/`--tint`, `--neutral`,
    `--sample`, `--suggest`);
  - `tests/v0110_camera_profiles.rs`.

  Decision: one tint unit is 1/3000 in uv, the DNG SDK's scale; the earlier
  "±0.0003" wording was corrected in `docs/photography-v1.md`. The kernels are
  documented there. The stage-3 transform is applied to pixels once rendering
  exists (items 5–6). The checkbox stays open until review and hosted CI pass.
- [ ] **5. Add lens and geometry correction.** Cover distortion, vignetting,
  chromatic aberration, perspective, leveling, rotation, and constrained crop.
  *Status: implemented for review.* This item has five parts:
  - lens profiles and the bounded LCP subset in `src/photo/lens.rs`;
  - the lens chain, geometry, crop, single-pass warp and upright analysis in
    `src/photo/warp.rs`;
  - the stage 1–5 pipeline in `src/photo/pipeline.rs`, with `lens`, `geometry`
    and `crop` validation in `src/photo/develop.rs`;
  - `pentool photo profile add --lens`, `pentool photo profile import-lcp`,
    `pentool raw develop` (`--lens-profile`, `--set`/`--unset`, `--upright`,
    `--guide`), `pentool photo info` and `pentool photo render`;
  - `tests/v0110_lens_geometry.rs`.

  Decisions:
  - `crop.aspect` shapes only a constrained crop;
  - `crop.rect` validation is stricter than the schema (positive size, inside
    the frame);
  - guides are given in the lens-corrected oriented frame;
  - `photo render` handles raw sources only until rendered sources can be added.

  The checkbox stays open until review and hosted CI pass.
- [ ] **6. Build the development stack.** Add exposure, highlights/shadows,
  whites/blacks, curves, clarity, texture, dehaze, vibrance, HSL, grading,
  monochrome, grain, vignette, calibration, and stable processing order.
  *Status: implemented for review.* This item has five parts:
  - the stage 6–10 kernels and calibration in `src/photo/adjust.rs`, specified
    in "Development stack" in `docs/photography-v1.md`;
  - calibration composed into the stage-3 transform in `src/photo/profile.rs`,
    with stages 6–10 plus the auto tone and airlight analyses in
    `src/photo/pipeline.rs`;
  - validation of `tone`, `presence`, `curves`, `hsl`, `grading`,
    `monochrome`, `effects` and `calibration` in `src/photo/develop.rs`;
  - `pentool raw develop --exposure` and `--auto-tone`, with automatic airlight
    resolution in `src/photo/catalog.rs`;
  - `tests/v0110_development.rs`.

  Decisions:
  - highlights and shadows are global, ratio-preserving tone operators;
  - color works in Oklab built on the working space;
  - monochrome replaces HSL, vibrance and saturation;
  - stage 9 runs monochrome or HSL, then vibrance, saturation and grading;
  - the dehaze airlight is resolved by `raw develop` and stored, so rendering
    never re-analyzes;
  - the stage-11 shoulder and gamut mapping stay with item 10.

  The checkbox stays open until review and hosted CI pass.
- [ ] **7. Add detail processing.** Implement luminance/color noise reduction,
  sharpening, moiré reduction, defective-pixel handling, and defringe.
  *Status: implemented for review.* This item has four parts:
  - the kernels in `src/photo/detail.rs`, specified in "Detail" in
    `docs/photography-v1.md`;
  - defective pixels in stage 1 (`raw::decode_with`), stage 2 in
    `pipeline::decode_working`, and capture sharpening at the end of stage 9;
  - validation of `detail` in `src/photo/develop.rs`;
  - `tests/v0110_detail.rs`.

  Decisions:
  - detail radii are in developed pixels, because detail targets the sensor's
    pixel scale;
  - defective-pixel `list` coordinates are active-area pixels, and an
    out-of-range point fails the render;
  - stage 2 judges color in Oklab of the profile matrix's ProPhoto, and
    defringe hue sliders map onto Oklab hue windows;
  - capture sharpening works on log luminance, so it is independent of
    exposure and keeps hue.

  The checkbox stays open until review and hosted CI pass.
- [ ] **8. Add local adjustments.** Reuse gradient, radial, range, depth, and
  painted masks. AI-derived masks must first become materialized editable masks.
  *Status: implemented for review.* This item has five parts:
  - validation and rendering in `src/photo/local.rs`, specified in "Local
    adjustments" in `docs/photography-v1.md`;
  - stage hooks in `adjust::apply_with` (after tone, presence, color and
    capture sharpening), with coverage computed in uncropped frame coordinates;
  - `photo mask paint`, which paints brush components with the raster brush
    engine into `raster_tiles`, with the tiles retained by garbage collection;
  - `raw develop` resolving the dehaze airlight for a local dehaze;
  - `tests/v0110_local.rs`.

  Decisions:
  - depth components are refused with `[unsupported-capability]`, because this
    build pairs no depth maps with sources;
  - `mask` components accept raster mask resources only, which is how AI-derived
    masks enter; vector resources are refused;
  - range-luminance uses Oklab lightness, and range-color uses an Oklab
    distance with an `amount`-scaled tolerance;
  - signed noise, moiré and defringe extrapolate away from the reduced image,
    and negative sharpness is a blur.

  The checkbox stays open until review and hosted CI pass.
- [ ] **9. Add HDR and panorama merging.** Provide bounded alignment, deghosting,
  projection, seam blending, attribution, cancellation, and non-destructive output.
  *Status: implemented for review.* This item has five parts:
  - the merges in `src/photo/merge.rs`, specified in "HDR and panorama merges"
    in `docs/photography-v1.md`;
  - the derived DNG writer and transparency reader in `src/photo/dngout.rs`,
    with the pre-balance clip map from `raw::decode_with` and transparency
    warped into alpha by `pipeline::develop`;
  - `catalog::merge`, which records `derived` attribution and verifies derived
    facts against their bytes;
  - `pentool photo merge-hdr` and `photo merge-pano` (`--scale`, `--settings`,
    `--external`, `--dry-run`), committing an external DNG and the document
    together;
  - `tests/v0110_merge.rs`.

  Decisions:
  - inputs read stages 1–3 from their master variant;
  - the output limit is also capped by the develop budget (about 41
    megapixels);
  - `--settings first` copies look settings only (not raw, white balance,
    calibration, stage-2 detail, lens, geometry or local; panoramas also skip
    crop);
  - panoramas do not compensate exposure between frames;
  - the EXIF focal length assumes a 36 mm long edge.

  The checkbox stays open until review and hosted CI pass.
- [ ] **10. Add wide-gamut and HDR RGB delivery.** Define primaries, transfer
  functions, inspection, tone mapping, SDR conversion, metadata, and validation.
  *Status: implemented for review.* This item has five parts:
  - stage 11 in `src/photo/output.rs` (look table and tone curve, the shoulder
    scaled to the output peak, perceptual or relative-colorimetric gamut
    mapping, quantization, measurement, chunk generation and verification),
    specified in "Wide-gamut and HDR delivery" in `docs/photography-v1.md`;
  - the camera profile's `ProfileLookTable*` and `ProfileToneCurve` in
    `src/photo/profile.rs`, carried by `pipeline::Developed::rendering`;
  - generated ICC v4 display profiles in `src/photo/icc.rs`, and tagged PNG
    writing and chunk reading in `src/photo/png.rs`;
  - `pentool photo render` with `--intent`, `--hdr pq|hlg` and `--headroom`,
    reporting `delivery`, and the new `pentool photo inspect`;
  - `tests/v0110_delivery.rs`.

  Decisions:
  - SDR white is 203 cd/m²; HLG puts it at signal 0.75, which limits HLG
    headroom to about 1.92 stops, and measures light at a 1000 cd/m² display;
  - perceptual mapping touches only out-of-gamut pixels, so in-gamut values
    round-trip exactly;
  - HDR output carries `cICP`, `mDCV` and `cLLI` but no ICC profile; SDR carries
    ICC and, where H.273 has codes, `cICP`;
  - the editor's Display P3 preview and overlays move to item 15.

  The checkbox stays open until review and hosted CI pass.
- [ ] **11. Add variants, snapshots, and synchronized edits.** Virtual copies do
  not duplicate sources; synchronization supports selected settings and exceptions.

  *Status: implemented for review.* This item has four parts:
  - `src/photo/variants.rs`: variant add (from a variant or a snapshot), rename
    and remove; snapshot add, restore and remove; and settings sync, specified in
    "Photo entries, variants and snapshots" and "Synchronized settings" in
    `docs/photography-v1.md`;
  - `pentool photo variant`, `photo snapshot` and `photo settings sync`, each one
    transaction with `--dry-run` and `--if-revision`;
  - unit tests of group selection and skip rules in `src/photo/variants.rs`;
  - `tests/v0110_variants.rs`.

  Decisions:
  - sync copies whole groups; a group the source does not set is removed from
    the target, and `process` is never synchronized;
  - `--auto-per-photo` re-runs gray-world white balance, upright and auto tone
    per target; a dehaze airlight is always measured again on a raw target whose
    inputs changed, because it is a measurement of the image;
  - `raw` is also skipped when its stored camera profile is for another camera;
  - removing a variant with snapshots or photo nodes is refused rather than
    cascading;
  - selection-query targets are refused until item 12 adds search.

  The checkbox stays open until review and hosted CI pass.
- [ ] **12. Add local organization.** Provide ratings, picks, labels, keywords,
  stacks, contact sheets, compare/survey, and search without a required database.

  *Status: implemented for review.* This item has four parts:
  - `src/photo/organize.rs`: the search query language, ratings, picks, labels,
    keywords, stacks, manual and smart collections, and the contact sheet and
    compare builder, specified in "Stacks, collections and search" in
    `docs/photography-v1.md`;
  - `pentool photo search`, `photo rate`, `photo keyword`, `photo stack`,
    `photo collection`, `photo contact-sheet` and `photo compare`; every edit is
    one transaction with `--dry-run` and `--if-revision`, and `photo settings
    sync --to` now accepts a query;
  - catalog validation of cross-stack membership, keyword case and smart queries,
    and unit tests of queries, globs and selections in `src/photo/organize.rs`;
  - `tests/v0110_organize.rs`.

  Decisions:
  - keywords are compared with simple Unicode lowercasing and stored as given,
    not NFC-normalized, to avoid a new dependency; the spec says so;
  - a manual collection given a query freezes its matches into IDs; smart
    collections are evaluated on use, nest at most 8 deep and refuse cycles;
  - contact sheets and compare share one layout and never change the document;
    photos without a thumbnail become reported gray cells rather than failing.

  The checkbox stays open until review and hosted CI pass.

- [ ] **13. Add metadata privacy.** Support a deliberate EXIF/IPTC/XMP subset and
  export policies for copyright, keywords, GPS, identity, serials, and timestamps.

  *Status: implemented for review.* This item has four parts:
  - `src/photo/metadata.rs`: categories, policies, the source reader for DNG/TIFF,
    JPEG and PNG, a deterministic EXIF and XMP writer, PNG and JPEG embedding,
    redaction and the privacy report, specified in "Metadata privacy" in
    `docs/photography-v1.md`;
  - `pentool photo metadata`, `photo privacy-report` and `photo describe`, and
    `photo render --metadata`, `--metadata-include` and `--metadata-exclude`;
  - a privacy warning in `package pack` for asset documents whose photo sources
    carry GPS, serials or identity;
  - unit tests of policies and writer round trips, and `tests/v0110_metadata.rs`.

  Decisions:
  - the category table is the implemented subset (no rights usage terms, creator
    contact or headline); the spec now says so;
  - source XMP is never copied; it is scanned for private markers only, and
    pentool writes its own packet; compressed XMP counts as `identity`;
  - non-ASCII text goes to XMP only; Software is never written alone;
  - the JPEG writer and TIFF embedding are used by item 14's export; item 13
    exposes PNG through `photo render`.

  The checkbox stays open until review and hosted CI pass.

- [ ] **14. Add output recipes and batch delivery.** Cover web, social, archive,
  and photo-lab dimensions, RGB profiles, precision, codec, naming, and sharpening.

  *Status: implemented for review.* This item has four parts:
  - `src/photo/export.rs`: recipe validation and the four built-in recipes, the
    output frame of every resize mode and print fit, Lanczos-3 resizing in linear
    light, output sharpening, naming, the planner and the staged batch run,
    specified in "Output recipes and batch delivery" in `docs/photography-v1.md`;
  - `src/photo/jpeg.rs`, a deterministic baseline JPEG encoder with 4:2:0 or 4:4:4
    chroma, JFIF density and ICC segments, and a single-strip TIFF writer that
    carries the metadata through `metadata::tiff_file`;
  - `pentool photo export` and `photo recipe list|set|remove`; document recipes
    are validated with the document;
  - unit tests of recipes, frames, resampling and naming, and
    `tests/v0110_export.rs`.

  Decisions:
  - pentool encodes JPEG itself because the bundled encoder cannot subsample
    chroma and the recipe contract needs `420` and `444`;
  - resizing happens in linear working light before stage 11, so the output tone
    curve, gamut mapping and quantization see the final pixels; overshoot is
    clamped, so edges do not ring;
  - a print accepts a one-pixel difference per side as the same aspect, turns to
    match the photo, pads with white, and enlarges by default; `--print` and
    `--fit` supply the size and fit per run;
  - same-name outputs are always `[conflict]` (`--overwrite` only replaces files
    from earlier runs); `--selection picks` in the spec became a query such as
    `collection:picks`; export is single-worker.

  The checkbox stays open until review and hosted CI pass.

- [ ] **15. Build photographer UX.** Add filmstrip/grid, loupe, culling, histogram,
  clipping warnings, development panels, masks, crop, compare, and copy/sync.

  *Status: implemented for review.* This item has four parts:
  - `src/photo/studio.rs`: previews and edits. A preview is developed, resized
    in linear light and run through stage 11 into sRGB or Display P3 with
    matching `iCCP`/`cICP`; it counts clipping and paints the clipping, gamut
    and mask overlays. An edit dispatches develop, rate, keyword, variant,
    snapshot, restore, sync and paint (including creating a brush adjustment)
    to the existing engines. Supporting changes are `output::render_marked`,
    `pipeline::develop_mask` and `catalog::render_mask`/`brush_plane`;
  - `/api/photo/catalog`, `detail`, `preview` and `edit` in `src/server.rs`;
    edits are revision-guarded transactions with dry run and undo;
  - `web/photo-panel.js` and the Photos studio in `web/index.html` and
    `web/style.css`: grid/filmstrip, loupe, compare, culling keys, histogram
    with clipping indicators, overlays, white balance and development sliders,
    crop, radial/linear/brush masks, copy/sync, variants, snapshots and
    keywords; specified in "Photo editor" in `docs/photography-v1.md`;
  - unit tests in `studio.rs` and `tests/v0110_studio.rs`, which cover the
    preview tags, overlays, every panel's edit, refusals, brush strokes and the
    HTTP endpoints with a stale-revision conflict.

  Decisions:
  - the browser does no develop math and never writes the document. Previews
    are Rust renders, so what the studio shows is what export produces;
  - the preview is 8-bit and perceptually mapped. The space defaults to Display
    P3 when the display reports P3. HDR display preview stays out of scope;
  - brush strokes from the studio accumulate into the adjustment's last brush
    component on the current plane, so erasing works and a session never hits
    the 16-component limit. The CLI still adds a component unless
    `--component` is given;
  - the studio is a full-screen overlay rather than inspector panels, because
    the 272-pixel inspector cannot hold a loupe and filmstrip; it captures
    keys while open;
  - grain from the slider gets seed 1 when the variant has none.

  The checkbox stays open until review and hosted CI pass.
- [ ] **16. Prove conformance and performance.** Test hostile RAWs, profiles,
  precision, corrections, masks, merges, privacy, rollback, packaging, and caches.

  *Status: implemented for review.* This item has five parts:
  - `src/photo/cache.rs`: the preview cache specified in "Caches". Entries are
    content-addressed PNGs carrying a CRC-checked `pnCk` key chunk, written
    atomically and bounded by `PENTOOL_PHOTO_CACHE_BYTES` with LRU eviction.
    It is used by the studio (`serve --no-cache` turns it off) and by the new
    `photo preview` and `photo cache clear` commands;
  - `pentool benchmark --photo` in `src/benchmark.rs`: ingest, cull, repeated
    full-size develop, cold and cached preview, and batch export of a synthetic
    Bayer shoot, with limits and `--max-ms`. Measurements are in
    `docs/performance.md`;
  - `tests/v0110_conformance.rs`: hostile DNG, DCP, lens-profile and PNG
    mutation sweeps; pinned develop and HDR-merge digests; rollback of about
    thirty failing commands; the cache contract; and a packaged shoot that
    reproduces its exports byte for byte after installation elsewhere;
  - `photo::cancellation` unit tests: decode and development, an external HDR
    merge, and exports into new and existing directories are cancelled at
    every checkpoint without mutation or orphan files;
  - a bug the sweep found: a hostile `.dcp` whose table dimensions overflowed
    `usize` panicked in `src/photo/profile.rs`. The sizes now saturate and are
    refused with `[limit-exceeded]`, with a regression test.

  Decisions:
  - cancellation at each checkpoint is driven by a `#[cfg(test)]` counter in
    `composite::check_cancelled`, so release builds carry no test hook;
  - the shared transaction's "revision mismatch" error keeps its wording and
    maps to `revision_conflict` as before, and the rollback test accepts it;
  - the golden digests were pinned on Windows. Linux and macOS agreement is
    proven only when hosted CI passes;
  - benchmark figures come from one developer machine and are not a hosted-CI
    claim;
  - open contract question, not implemented: page rendering of a `photo` node
    still fails with `[unsupported-capability]`, as "Until a build includes the
    development pipeline" in `docs/photography-v1.md` allows. Whether v0.11.0
    must render photo nodes on pages needs a decision before release.

  The checkbox stays open until review and hosted CI pass.

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
