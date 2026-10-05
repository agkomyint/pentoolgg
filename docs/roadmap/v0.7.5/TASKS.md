# v0.7.5 — Professional color management and print production

Extend v0.7.4's RGB foundation into professional ICC, CMYK/Lab, proofing, spot
color, separation, preflight, and standards-conforming print delivery in Rust.

## Product contract

- Working, proof, display, and output profiles have distinct explicit roles.
- Profiles and output conditions are validated, content-addressed dependencies.
- RGB masters remain editable; press conversion is normally an output recipe.
- Unsupported color, transparency, font, or PDF constructs fail preflight.
- Pentool claims conformance only when an independent validator confirms it.

## Must ship, in order

- [ ] **1. Freeze the professional color specification** with ICC scope,
  transforms, intents, adaptation, proofing, channels, overprint, and fixtures.
- [ ] **2. Complete a bounded Rust ICC engine** or integrate an audited Rust
  implementation without silently adding a native runtime dependency.
- [ ] **3. Add profile management:** import, inspect, validate, assign, convert,
  embed, collect, hash verification, licensing, and missing-profile diagnostics.
- [ ] **4. Extend `.pen` color declarations** across nodes, styles, gradients,
  images, effects, blends, packages, diff, and legacy-sRGB migration.
- [ ] **5. Add CMYK and Lab workflows** with channel-aware values, sampling,
  adjustments, gradients, conversion, and documented compositing behavior.
- [ ] **6. Add black and ink analysis:** TAC, rich/registration black, neutral
  construction, small-text warnings, and profile-derived separation reporting.
- [ ] **7. Add soft proofing** with gamut warnings, paper/ink simulation,
  preserve-numbers behavior, comparison, and explicit display limitations.
- [ ] **8. Add spot colors and offline ink libraries** with tint, alternate color,
  provenance, aliases, conflicts, and package support.
- [ ] **9. Add overprint, knockout, and separations** with plate preview/export
  and explicit failure for unsupported transparency interactions.
- [ ] **10. Add production geometry** including bleed, slug, page boxes, marks,
  safe zones, and narrowly specified manual/automatic trapping.
- [ ] **11. Build supported PDF/X export** with output intent, fonts, boxes,
  transparency policy, deterministic structure, and independent validation.
- [ ] **12. Add structured print preflight** for profiles, inks, spots, fonts,
  resolution, bleed, transparency, metadata, and selected PDF/X rules.
- [ ] **13. Add proof/production packages** containing proofs, separations, ink
  maps, reports, outputs, and every verified dependency.
- [ ] **14. Add reusable print recipes** for profiles, intent, black policy,
  spots, resampling, compression, bleed, marks, PDF standard, and thresholds.
- [ ] **15. Build print-focused UX** for channels, separations, overprint, gamut,
  ink warnings, proof setup, bleed overlays, preflight navigation, and export.
- [ ] **16. Prove security, conformance, and performance** across hostile ICC/PDF
  inputs, transform vectors, plates, fonts, rollback, and all release targets.

## CLI direction

```sh
pentool color profile import campaign.pen ./press.icc --name press-coated
pentool proof campaign.pen --profile profile:press-coated --paper --ink-black
pentool inspect ink campaign.pen --profile profile:press-coated --limit 300 --json
pentool separation preview campaign.pen --plates process,spot --output proof.pdf
pentool preflight campaign.pen --recipe magazine-print --json
pentool export campaign.pen campaign.pdf --recipe magazine-print
```

## Acceptance targets

- Proof an RGB master without destructively converting it.
- Reproduce process and spot plate values across supported targets.
- Detect ink, bleed, resolution, font, transparency, and profile failures.
- Produce a file passing the claimed PDF/X conformance validator.
- Collect every dependency and reproduce production output offline.
- Reject malformed inputs without fallback, partial output, or unbounded work.

## Acceptance demo

Open the v0.7.4 wide-gamut master, assign a press condition, add a spot ink,
configure overprint, proof paper/ink/gamut/TAC, resolve structured preflight issues,
inspect every plate, and export validated PDF/X plus proof and report. Collect the
project and reproduce the declared production output offline on another target.
