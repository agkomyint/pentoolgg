# v0.12.0 — Professional color management and print production

Extend v0.11.0's RGB foundation into professional ICC, CMYK/Lab, proofing, spot
color, separation, preflight, and standards-conforming print delivery in Rust.

Implementation has started from the v0.11.1 codebase. The developing contract is
[`professional-color-v1.md`](../../professional-color-v1.md). It describes the
target behavior, not released capabilities. Items remain unchecked until their
implementation and required evidence are complete.

## Implementation handoff (2026-10-10)

The package remains `0.11.1`; this roadmap does not authorize a version bump,
tag, or release. Top-level milestone boxes stay open until their complete ship
criteria and cross-platform evidence exist. The smaller boxes below record code
that is implemented and locally exercised so another contributor can resume at
the first unchecked boundary.

### Status at a glance (2026-10-10)

- No top-level task is complete. Tasks 1-8 are partially implemented; tasks
  9-16 have only the shared ICC, catalog, role, package and performance
  foundations. Rough overall progress: about one third.
- Resume order: all task 5 boundaries are done and task 9 has a first plate-separation
  subset; task 11 has a flattened PDF/X-4 draft exporter; continue with its validator evidence and vector output, then the open parts of task 9. Tasks 6-8, 10 and 12-15 mostly build on those.
- Binary size: the local Windows release binary is 24,247,808 bytes. The owner
  raised the release gate from 24,000,000 to 30,000,000 bytes on 2026-10-10
  (`.github/workflows/ci.yml`, `release.yml`, `docs/performance.md`), so this
  is no longer a blocker; keep task 16 within the new gate.
- Repository state: the v0.12.0 work (`src/color/`, `src/composite/`,
  `tests/v0120_*.rs`, `docs/fixtures/print-*`, `docs/professional-color-v1.md`,
  `docs/pen-format-v8.schema.json`, `docs/color-value-v1.schema.json`,
  `scripts/*print*` and the modified shared modules) is uncommitted on top of
  the `v0.11.1` tag. Commit it to a feature branch before large refactors, and
  stage by explicit path: the working tree also holds unrelated user files
  (`examples/`, `output/`, bug folders, later roadmaps).

Completed foundation:

- [x] Bounded ICC v2/v4 parsing for supported RGB, gray and CMYK matrix/TRC and
  LUT paths, strict intent selection, device links, D50/Lab math and hostile
  input limits.
- [x] Content-addressed v8 profile catalog, explicit roles, import/inspect/
  validate/embed/collect/remove, transactional paint assignment/conversion and
  dependency verification in offline packages.
- [x] Native named RGB, ICC RGB/gray/CMYK and physical D50 Lab values with
  separate alpha across supported scene paint fields and gradients.
- [x] Prepared native solid and linear/radial/conic fill evaluation into bounded
  `f64` named-linear-RGB surfaces without an intermediate RGB8 conversion.
- [x] Shared ordered scene renderer float target for backgrounds, fills, affine
  placement, isolated/pass-through groups, instance fallbacks, opacity,
  consecutive clipping and released encoded/linear blend-space semantics.
- [x] Bounded sRGB8 image, raster and developed-photo interpretation into the
  selected working primaries before sibling blending.
- [x] Reusable vector/raster masks applied as normalized float coverage while
  retaining working RGB channel precision.
- [x] Focused float scene coverage in `tests/v0120_working_scene.rs` (13 tests),
  including low-opacity stacks, clipping, groups, alternate primaries, CMYK
  intent paths, cancellation, limits, raster input, masks, native vectors and
  antialiased text.
- [x] All 13 existing adjustment kinds now execute on working float surfaces
  with scopes, masks, declared blend coordinates, atomic failure and row-level
  cancellation. Encoded-sRGB parameter meaning is retained across working
  primaries and extended-channel policies are documented.
- [x] A bounded read-only ink-analysis library reports profile-derived CMYK
  paint samples, TAC and channel extrema, black/neutral classifications and
  small inline-text warnings with deterministic pagination.
- [x] A bounded sample-level soft-proof library reports proof-device round trips,
  transform clipping, Delta E 76 gamut warnings, exact-profile preserve-numbers,
  absolute-intent media-white simulation and display conversion limitations.
- [x] A bounded offline spot-ink library provides stable IDs, provenance,
  aliases and collision checks, explicit tint declarations, validated preview
  alternates and hash-pinned package snapshots.
- [x] Float color-overlay and gradient-overlay effects reuse native paint and
  gradient evaluation while preserving source silhouettes, effect order,
  content/final opacity, declared blend coordinates, budgets and atomic failure.
- [x] `inspect ink` exposes the bounded paint-sample analysis as a read-only,
  paginated CLI with explicit thresholds and optional verified CMYK profile.
- [x] Optional v8 spot libraries are validated with ICC alternates and collected
  into a backward-compatible, hash-audited `.penpkg` manifest table; packaging
  requires explicitly allowed redistribution.

Next implementation boundary for task 5:

- [x] Render vector and text coverage/color into working float surfaces without
  lowering native paints through the ordinary RGB8 preview.
- [x] Port the 13 adjustment operations to channel-aware float processing and
  preserve scope, masks, blend spaces and atomic failure.
- [x] Finish active effects and non-affine transform resampling on bounded float
  surfaces. Color and gradient overlays, blur, shadows, glows, stroke,
  non-affine resampling and pattern fills (sRGB8 raster interpretation) are
  complete.
- [x] Define and test the extended-gamut policy for every non-normal blend mode
  (`blend::extended_domain`; nine total separable modes extend unclamped,
  dodge, burn and the four non-separable modes refuse explicitly).
- [x] Add the production output transform and quantize only at an explicitly
  selected delivery boundary (`color::output`, `tests/v0120_output.rs`);
  ordinary exports remain the documented RGB8 preview.
- [x] Verify the current raster/mask slice: formatting and Clippy with warnings
  denied are clean; the locked all-target suite passes 668 tests in 71 suites
  with 0 failures and 3 ignored tests.
- [x] Verify the vector/text slice with 13 focused float-scene tests and Clippy
  with warnings denied; the locked all-target suite now passes 670 tests in 71
  suites with 0 failures and 3 ignored tests.
- [x] Verify the integrated adjustment, ink, proof and spot foundations:
  formatting and all-target Clippy with warnings denied are clean; the locked
  all-target suite passes 691 tests with 0 failures and 3 ignored tests.
- [x] Verify the overlay, ink CLI and spot package integration: formatting and
  all-target Clippy with warnings denied are clean; the locked all-target suite
  passes 702 tests with 0 failures and 3 ignored tests.

Tasks 6-8 now have the bounded library foundations described below. They remain
open because page-area analysis, rendered proof UI/output, scene spot paints,
separations and complete package integration are not implemented. Tasks 9-16
remain at the shared ICC, catalog, role, package and performance foundations.

## Product contract

- Working, proof, display, and output profiles have distinct explicit roles.
- Profiles and output conditions are validated, content-addressed dependencies.
- RGB masters remain editable; press conversion is normally an output recipe.
- Unsupported color, transparency, font, or PDF constructs fail preflight.
- Pentool claims conformance only when an independent validator confirms it.

## Must ship, in order

- [ ] **1. Freeze the professional color specification** with ICC scope,
  transforms, intents, adaptation, proofing, channels, overprint, and fixtures.
  In progress: `docs/professional-color-v1.md` defines the target roles, limits,
  migration, plate semantics, delivery and evidence gates. The authored
  `print-rgb-linear-v2.icc`, `print-rgb-linear-v4.icc` and
  `print-cmyk-lut16-v4.icc` probes and their
  `print-conformance.json` manifest establish initial transform fixtures.
  A developing v8 catalog schema and hash-pinned `print-v8.pen` migration fixture
  now exist. `scripts/check-print-schema.py` independently validates pinned scenes
  and positive/negative native sample/destination grammar cases without fetching
  schemas. The pinned `print-v8-roles.pen` adds explicit target settings and an
  embedded authored RGB monitor probe while leaving proof unconfigured.
  The complete native scene schema, plate fixtures and wider reference
  evidence are still required before freezing the contract.
- [ ] **2. Complete a bounded Rust ICC engine** or integrate an audited Rust
  implementation without silently adding a native runtime dependency.
  In progress: `src/color/icc.rs` reads bounded ICC.1 v2/v4 containers and
  compiles RGB/gray TRC and `mft1`/`mft2`/`mAB`/`mBA` RGB/gray/CMYK LUT paths using existing
  deterministic photography math. The intent compiler selects explicit paths,
  applies absolute media-white scaling and reports matrix/TRC equivalence;
  device links require a matching recorded intent. `tests/v0120_icc.rs` and
  `tests/v0120_lut.rs` cover bytes, vectors, PCS encodings, plate channel order,
  malformed inputs and refused substitution. Development-only LittleCMS checks
  independently evaluate the authored probes and all six paths of an unmodified
  licensed CGATS21_CRPC7 press profile. CLUTs use tetrahedral 3D interpolation
  and linear interpolation between 4D slices. The separate link planner composes
  the selected paths and records destination-driven v4 perceptual/saturation
  black mapping, including detected v2 source black. Tests cover all four intents
  against independent press/PCS and RGB/press vectors plus a v2 CMYK black-L*=10
  probe. Wider independent vectors and cross-platform evidence remain open.
  These engine vectors do not establish production conformance.
- [ ] **3. Add profile management:** import, inspect, validate, assign, convert,
  embed, collect, hash verification, licensing, and missing-profile diagnostics.
  In progress: `color profile inspect FILE` reports hashes, bounded localized
  metadata, paginated tag headers and explicit executable capabilities;
  `color profile validate FILE --direction ... --intent ...` requires the
  requested path. Both are read only and do not certify ICC conformance.
  Explicit v8 migration and document `import`, `list`, `show`, and `embed` now
  share a bounded content-addressed catalog. Original embedded/external bytes,
  headers, metadata, aliases and caller-declared redistribution are verified;
  transaction tests cover dry runs, revision guards, rollback, undo and downgrade
  refusal. Unused profiles preserve rendered pixels and photography edits retain
  v8. `color paints assign` and `convert` plan transactional stored-paint edits,
  with explicit shared-resource consent, locks across pages, bounded change
  reports and original-byte revision guards. Vector mask edits replace immutable
  content identities and all affected aliases/attachments atomically.
  `color profile collect` embeds all verified ICC dependencies with explicit
  local/sharing policy; existing packages pin and reverify their ICC audit tables.
  Unused-profile removal refuses references across the whole document. Explicit
  working/proof/display/output target settings validate required paths, pin ICC
  identities and are consumed by sample evaluation and `paints convert --role`.
  Roles participate in removal, downgrade, collection and package validation.
  Role-aware production compositing/proof/export, raster conversion,
  production dependency integration and cross-platform evidence remain open.
- [ ] **4. Extend `.pen` color declarations** across nodes, styles, gradients,
  images, effects, blends, packages, diff, and legacy-sRGB migration.
  In progress: a shared native sample grammar/evaluator defines named RGB,
  ICC RGB/gray/CMYK and physical D50 Lab, with strict channel/profile checks and
  separate alpha. Native vector/background paints, solid fills, native gradient
  stops and explicit interpolation destinations now validate and render through a shared supported-field
  traversal, including named styles, component roots, instance fallbacks and
  immutable vector masks.
  Tests cover exact pixels, ICC channels, page-scoped missing external profiles,
  bounded traversal, rollback and downgrade refusal. Raster interpretation,
  override planning, package/diff integration and production compositing remain open.
- [ ] **5. Add CMYK and Lab workflows** with channel-aware values, sampling,
  adjustments, gradients, conversion, and documented compositing behavior.
  In progress: read-only `color evaluate` reports original channels, D50 XYZ,
  selected ICC paths and optional converted channels/clipping. All-intent
  RGB/press numeric conversion is checked against independent reference vectors.
  `color interpolate` now samples explicit native channels through the shared
  evaluator: converted endpoints, four-channel same-profile CMYK, physical Lab
  and separate straight alpha, with bounded input and read-only reports.
  Explicit v8 linear/radial/conic fill gradients and overlays now interpolate
  native channels before preview conversion, with prepared endpoints, cached
  selected paths and separate alpha. A pinned three-ramp fixture and independent
  RGB/Lab pixel assertions cover preview behavior; real press tests cover all
  four intents. Destination references participate in removal, downgrade and
  offline package checks. Legacy gradient pixels remain covered unchanged.
  The shared blend kernel now also supports bounded float linear-RGB surfaces,
  precise coverage and native-paint compositing through the evaluator, with
  explicit working primaries, signed/extended normal blending and source-path
  clipping reports. The shared fill geometry loop now also renders native solid
  and linear/radial/conic paints directly into working float coverage, retaining
  channel/alpha precision and extended gamut. String-space gradients preserve
  their existing interpolation domains and report conversion clipping. Float
  fill tests cover independent RGB/Lab/geometry math, all press intents, bounds,
  live tokens and cancellation. The shared ordered scene renderer now also has
  a developing float target for native vector/text fill and stroke planes,
  fills/backgrounds, affine placement, groups,
  instance fallbacks, opacity and precise clipping chains. Bounded sRGB8 image,
  raster and developed-photo pixels now convert to working storage before scene
  blending; reusable vector/raster masks operate as coverage without quantizing
  working color. It consumes the working role/intent, preserves declared
  blend-space semantics, and accounts for float pixels/coverage and conversion
  scratch against the aggregate memory limit. Tests cover low-opacity stacks,
  isolation, working primaries, raster interpretation, masks and atomic
  failure/cancellation. The extended-gamut policy of every non-normal blend
  mode is fixed and tested, and `color::output` is the explicit delivery
  boundary that converts a float render to quantized 8/16-bit device samples.
  Scene sampling and CLI/export integration of that boundary remain open;
  current ordinary exports use an ephemeral sRGB8 preview.
  Float color and gradient overlays now run in the shared renderer with native
  paint precision, source silhouettes, declared blend coordinates and bounded
  atomic stacks. Spatial effects (blur, shadows, glows, stroke) now run on float
  surfaces with released geometry and byte-engine parity tests. Non-affine
  transform stacks resample float surfaces through the shared inverse mapper and
  bilinear kernel. Pattern fills sample into working RGB.
- [ ] **6. Add black and ink analysis:** TAC, rich/registration black, neutral
  construction, small-text warnings, and profile-derived separation reporting.
  In progress: `src/color/ink.rs` analyzes bounded visible paint and gradient-stop
  samples through an explicit CMYK output role. It reports TAC, per-channel
  extrema, rich/registration black, source-neutral multi-ink and inline small-text
  warnings using explicit recipe thresholds, with deterministic pagination and
  no mutation. These are declared paint samples, not rendered area or plate
  coverage; clipping, masks, effective opacity, shared-style text sizing, final
  compositing and plate export remain open. `inspect ink` exposes the current
  sample report with explicit thresholds, profile selection and pagination.
- [ ] **7. Add soft proofing** with gamut warnings, paper/ink simulation,
  preserve-numbers behavior, comparison, and explicit display limitations.
  In progress: `src/color/proof.rs` evaluates bounded native samples through an
  explicit proof profile and display role, reports proof-device round-trip
  Delta E 76 and transform clipping, preserves numbers only for the identical
  device profile, and exposes absolute-intent media-white scaling. The current
  ink simulation is the ICC device-to-PCS round trip. Scene proof images, UI,
  physical display calibration and spectral paper/ink simulation remain open.
- [ ] **8. Add spot colors and offline ink libraries** with tint, alternate color,
  provenance, aliases, conflicts, and package support.
  In progress: `src/color/spot.rs` defines bounded deterministic ink libraries
  with stable identity, case-insensitive alias conflict detection, caller-declared
  provenance/licensing state, native preview alternates, explicit tint/alpha and
  canonical hash-verified offline snapshots. Scene/catalog validation, `.penpkg`
  tint preview transforms, separations, overprint and plate output remain open.
  Optional v8 ink libraries now validate alternate ICC references and are
  collected into deterministic hash-audited package manifest tables; existing
  documents and manifests without inks remain compatible.
- [ ] **9. Add overprint, knockout, and separations** with plate preview/export
  and explicit failure for unsupported transparency interactions.
  In progress: `src/color/separation.rs` and `pentool color separate` produce
  deterministic C,M,Y,K + spot plates for flat opaque vector/text paints with
  knockout, overprint modes 0/1, spot tints and explicit refusal of unsupported
  transparency (tests/v0120_separation.rs). `pentool color separate --bits 16`
  writes 16-bit grayscale plates (report carries `bits`). `fill` nodes with
  opaque linear/radial/conic native-stop gradients separate by interpolating the
  stops' device values along an 8-bit position ramp (no trap, overprint or
  native interpolation space; partial-alpha stops refused). Plain `image`
  nodes separate by converting each rendered sRGB pixel through the
  destination profile (no overprint or trap; effects stay refused); placed
  `photo` nodes take the same 8-bit sRGB path after development. `--auto-trap WIDTH` (recipe `auto_trap`) applies a documented
  density-ordered automatic trap to the plates (tests/v0120_separation.rs).
  `--composite` writes a bounded composite plate preview (process plates
  through the output profile, spots as multiplied sRGB alternates;
  tests/v0120_separation.rs). Wide-gamut (non-sRGB8) photo separation and
  reference-RIP evidence remain open.
- [ ] **10. Add production geometry** including bleed, slug, page boxes, marks,
  safe zones, and narrowly specified manual/automatic trapping.
  In progress: `src/color/production.rs` and `pentool color production
  show|set|clear` implement validated trim/bleed/slug/art/safe geometry, PDF box
  conversion, mark geometry and painted bleed-band coverage
  (tests/v0120_production.rs). Crop and registration marks are now rendered
  as 0.25 pt hairlines in registration color (full ink on every process plate,
  not on spot plates) into the PDF/X raster and the plates; reports carry the
  mark count (tests/v0120_marks.rs). Manual trapping is specified and
  implemented in plate separations as `style.trap: {"width": units}` (a spread
  of the object's own ink, raising underlying plates only where higher, limited
  to 32 px, refused with overprint; tests/v0120_trap.rs). `marks.color_bars`
  prints eight process patches (C, M, Y, K, R, G, B, 50% K) in the bottom slug
  into the PDF/X raster and plates (tests/v0120_marks.rs). `production.slug_text`
  prints a validated single line in registration color in the top slug on
  the PDF/X raster and process plates (tests/v0120_slug.rs). Automatic trapping
  and slug text variables remain open.
- [ ] **11. Build supported PDF/X export** with output intent, fonts, boxes,
  transparency policy, deterministic structure, and independent validation.
  In progress: `src/color/pdfx.rs` and `pentool color pdfx` write a deterministic
  flattened-CMYK PDF/X-4 draft (output intent, embedded profile, XMP/Info, boxes
  from production geometry, atomic write, refusals before output;
  tests/v0120_pdfx.rs). `scripts/check-pdfx.py` (standard library only, no
  Pentool code) re-parses a draft and checks header, xref offsets, trailer IDs,
  output intent and ICC stream, Info/XMP version agreement, box nesting, stream
  lengths and sample counts; it passed on a generated draft and failed on five
  corruptions. It is structural evidence, not a conformance validator.
  Vector/live-transparency/font-embedding output, spot inks, and a named
  PDF/X validator result remain open; no conformance claim.
- [ ] **12. Add structured print preflight** for profiles, inks, spots, fonts,
  resolution, bleed, transparency, metadata, and selected PDF/X rules.
  In progress: `src/color/preflight.rs` and `pentool color preflight` report
  stable-coded errors/warnings for output profile, geometry, bleed coverage,
  nominal image resolution, transparency, text and unsupported spot/overprint
  use (tests/v0120_preflight.rs). A total ink limit (`--max-total-ink`, recipe
  `max_total_ink`, 100-400%) is measured on the delivered CMYK raster and reports
  `ink-total-exceeded`. Font embedding/licensing and metadata checks remain open.
- [ ] **13. Add proof/production packages** containing proofs, separations, ink
  maps, reports, outputs, and every verified dependency.
  In progress: `src/color/bundle.rs`, `pentool color package` and
  `package-verify` assemble the PDF/X draft, plates, preflight report, verified
  profiles, source document and a digest manifest atomically
  (tests/v0120_bundle.rs). A bounded soft-proof image (`proof.png`, numeric
  output-profile-to-display simulation, no paper model) and `ink-map.json`
  (per-plate coverage and heaviest total-ink pixel) are included, or recorded as
  skipped with a reason. Hash-verified linked images travel at their
  document-relative paths (manifest `assets`, role `image-asset`). Font
  collection remains open (`assets.fonts = "not-collected"`). Preflight now
  reads image nodes' `asset` field (it read a nonexistent `source`, so every
  page with an image failed with `image-unresolved`).
- [ ] **14. Add reusable print recipes** for profiles, intent, black policy,
  spots, resampling, compression, bleed, marks, PDF standard, and thresholds.
  In progress: `src/color/recipe.rs` and `--recipe FILE` on `color pdfx` and
  `color preflight` supply intent, ppi, minimum ppi, minimum bleed and bleed
  scale from a strictly validated JSON file (tests/v0120_recipe.rs). `profile`
  names the CMYK output destination (needs an intent; `--to` overrides it).
  `plate_bits` (8 or 16) and `--recipe` on `color separate` supply plate depth,
  intent and profile (tests/v0120_separation.rs); `auto_trap` and `plate_bits`
  also drive the plates of `color package` (tests/v0120_bundle.rs); `scale`
  (0.1-8) sets plate render scale for both commands. `allowed_spots` (up to 32
  ink IDs) makes `color preflight`/`package` report `spot-ink-not-allowed` errors
  for any other spot paint (tests/v0120_preflight.rs). `black_policy: "k-only-text"` reports
  `text-not-k-only` for text with C/M/Y in an ICC-CMYK fill; `require_marks`
  reports `marks-missing` for pages without declared marks. Spot
  `--recipe @NAME` reads a recipe embedded in the document's `print_recipes`.
  `compression` (`flate`|`none`) sets PDF stream compression
  (tests/v0120_pdfx.rs). `spot_policy: "reject"` turns PDF/X spot flattening into a preflight error
  (tests/v0120_preflight.rs). `spot_substitutions` (up to 16 `{"from":"to"}` ink IDs, no chains) moves every paint of one spot ink onto another ink's plate in memory for `color separate`, `package`, `preflight` and `pdfx`; an unknown ink is an error and the document on disk is untouched (tests/v0120_spot_substitution.rs). Resampling is governed by `ppi` alone while export is one
  flattened raster; per-image resampling and spot-to-process substitution remain open.
- [ ] **15. Build print-focused UX** for channels, separations, overprint, gamut,
  ink warnings, proof setup, bleed overlays, preflight navigation, and export.
  In progress: a "Print preflight" panel (`web/print-panel.js`) and read-only
  `GET /api/color/preflight` list stable-coded issues and resolved production
  geometry (tests/v0120_print_ui.rs). Read-only `GET /api/color/plates` (per-plate
  coverage, or one plate as a grayscale PNG) and `GET /api/color/proof` back a
  plate viewer and soft-proof button in the panel, and clicking a preflight
  issue dispatches `pentool-select-node` to select the object. The plate/proof view overlays
  the resolved media, bleed, trim and safe rectangles as SVG guides with a toggle
  (served markup asserted in the UI test). Overprinting objects are highlighted on the print view from the preflight report's
  `overprint_nodes` (top-level bounds; nested nodes are listed without bounds). `GET /api/color/gamut` and the "Gamut warning" button show pixels the output profile cannot reproduce in red (display-vs-soft-proof round trip, 12/255 channel threshold; numeric, not a proof). A proof-intent selector overrides the output role's rendering intent for the proof and gamut views (`intent` query). `GET /api/color/pdfx` and a download link export the PDF/X-4 draft from the panel. The print panel also publishes the resolved media, bleed, trim and safe boxes to the editor canvas (`pentool-print-guides` event, refreshed on each preflight; not live while editing). Proof setup is limited to the rendering intent (no simulate-paper or black-point controls) and export is the flattened draft only. There is no browser-level interaction test (endpoints and the
  served panel are tested; the DOM wiring is only syntax-checked).
- [ ] **16. Prove security, conformance, and performance** across hostile ICC/PDF
  inputs, transform vectors, plates, fonts, rollback, and all release targets.
  In progress: `docs/performance.md` records a same-machine optimized comparison
  of the shared blend kernel with identical output hashes and unchanged binary
  size. Float scene and plate performance and reference-RIP evidence remain open.
  Hosted CI run 38122650721 on `c4500ca` passed the test matrix on Ubuntu, Windows,
  Intel macOS and Apple Silicon macOS plus the policy, web, fuzz-smoke and
  binary-size jobs; the hosted Linux release build is 19,898,976 bytes against the
  30,000,000-byte gate (raised from 24,000,000 on 2026-10-10). The same run on the
  preceding commit caught a package staging-directory race that is now fixed with a
  regression test. The CI `binary-size` job now builds the release binary and
  applies the 30,000,000-byte gate on Ubuntu, Intel macOS, Apple Silicon macOS and
  Windows; per-target sizes are not recorded until that run completes, and the
  packaged release archives still have not been built.
  `tests/v0120_hostile_print.rs` covers oversized pages refused before render,
  truncated/corrupted PDF structures, and hostile recipes and package manifests
  (traversal, absolute, backslash, oversized), plus truncated and byte-flipped
  ICC profiles through catalog import into PDF/X and an RGB profile in the
  CMYK output role, and rollback (a failed PDF/X export or package leaves no
  file or directory, never replaces an existing output, and leaves the source
  document byte-identical). Local locked suite: 87 test binaries green, clippy
  and fmt clean. Font and hosted-target evidence remain open.

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

Open the v0.11.0 wide-gamut master, assign a press condition, add a spot ink,
configure overprint, proof paper/ink/gamut/TAC, resolve structured preflight issues,
inspect every plate, and export validated PDF/X plus proof and report. Collect the
project and reproduce the declared production output offline on another target.
