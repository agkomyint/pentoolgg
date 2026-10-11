# Pentool v0.12.0

Professional color management and print production foundation. Pentool now
manages ICC profiles, carries native CMYK/Lab/spot colors in the document,
separates plates, proofs and preflights a page, and exports a PDF/X-4 **draft**,
from the CLI or the print panel in the browser editor. The contract is
[`docs/professional-color-v1.md`](professional-color-v1.md); the roadmap is
[`docs/roadmap/v0.12.0/TASKS.md`](roadmap/v0.12.0/TASKS.md).

## Format

- `.pen` format version 8 adds a content-addressed ICC profile catalog, explicit
  color roles (display, output, proof), native color values, spot-ink libraries,
  per-page production geometry and print recipes. Schemas:
  `docs/pen-format-v8.schema.json`, `docs/color-value-v1.schema.json`.
- Older builds refuse v8 documents clearly and without modifying them. Existing
  sRGB documents keep their appearance.

## Color

- Bounded ICC v2/v4 parsing for supported RGB, gray and CMYK profiles; hostile or
  truncated profiles are refused.
- Native ICC RGB/gray/CMYK and D50 Lab values in fills, gradients, strokes and
  effects, with explicit intents and clipping reports.
- The renderer composites on float working surfaces; all 13 adjustments, masks,
  effects and blend modes run there and quantize only at the output transform.

## Print production

- `pentool color separate`: deterministic C, M, Y, K and spot plates with
  knockout, overprint and trapping, plus a composite preview.
- `pentool color preflight`: structured issues (output profile, geometry, bleed,
  resolution, transparency, unsupported constructs) that name the object and fix.
- `pentool inspect ink`: total ink coverage, rich black and neutral analysis.
- `pentool color production`: media, bleed, trim, slug, safe zone and marks.
- Soft proof with rendering-intent override, gamut-warning image, black point
  compensation and absolute-colorimetric paper-white simulation. These are
  numeric profile simulations, not a calibrated display or a contract proof.
- Offline spot-ink libraries with ICC alternates, and print recipes
  (`--recipe FILE` or `@NAME`) that carry profiles, intent and spot-to-spot ink
  substitution.
- `pentool color package` / `package-verify`: a production package with PDF/X
  draft, plates, preflight report, verified profiles and a digest manifest, built
  atomically and verified offline.
- Print panel in the editor: plates, proof, gamut, overprint highlight, intent
  and black point controls, canvas guides for media/bleed/trim/safe, and a PDF/X
  draft download.

## Known limitations (deferred)

- The PDF/X-4 output is a **flattened CMYK raster draft**. It makes no
  conformance claim and independent validation is reported as "not-run". Vector
  and embedded-font PDF/X output is deferred.
- No reference-RIP or external validator evidence is included yet.
- Spot-to-process substitution and per-image resampling in recipes are not
  implemented.
- Canvas guides refresh on each preflight rather than live while editing, and
  there is no browser-level interaction test.
- Float scene and plate performance has not had a dedicated optimization pass.

These are tracked in [`docs/roadmap/v0.12.0/DEFERRED.md`](roadmap/v0.12.0/DEFERRED.md).
