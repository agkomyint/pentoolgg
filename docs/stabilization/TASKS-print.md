# Stabilization tasks: v0.12 print, v0.13 painting, v0.14 layout

Hand-off for the agent fixing what `audit-print.md` found. No P0 defects were found.
Rules: patch release only, no new features, no push, local commits only. Regression
tests go in the files named below; run only the single test file you touched.

"patch-safe" means a bug/doc/test fix that needs no format or CLI contract change.
"needs decision" means the maintainer must pick between fixing the code and
narrowing the documented claim.

## PRINT-001  Studio print panel lost Classic print features (P1, needs decision)

- Repro: open `web-ui/src/workspaces/PrintPanel.tsx`; it offers preflight, "Load
  plates" and "Soft proof" only. `web-ui/src/lib/api.ts` wraps only preflight, plates
  and proof. `docs/roadmap/v0.12.0/TASKS.md` item 15, `docs/release-v0.12.0.md` and
  `docs/professional-color-v1.md` still describe gamut warning, PDF/X download link,
  proof intent/BPC selectors, bleed/trim/safe overlay (`pentool-print-guides`) and
  overprint highlight, and point at retired `web/print-panel.js`.
- Expected: documented controls exist in Studio, or docs say they do not.
- Actual: endpoints `/api/color/gamut` and `/api/color/pdfx` return 200 but no UI
  calls them.
- Suspected files: `web-ui/src/workspaces/PrintPanel.tsx`, `web-ui/src/lib/api.ts`,
  `web/studio.html` (rebuild with `bun run build`), docs above.
- Test to write: extend `tests/v0120_print_ui.rs`
  `studio_print_panel_calls_documented_endpoints` (assert the embedded
  `web/studio.html` contains `/api/color/gamut` and `/api/color/pdfx`), or a web-ui
  unit test if one exists.
- Acceptance: either every control listed in item 15 is reachable in Studio and the
  built `studio.html` is refreshed, or item 15, the release note and the color doc
  are corrected to the real control set and `web/print-panel.js` references removed.
- Decision: needs maintainer (restore feature versus narrow the claim). Doc-only
  option is patch-safe.

## PRINT-002  PDF/X output self-declares conformance with no validation (P2, patch-safe)

- Repro: `pentool color pdfx DOC out.pdf` (doc with output role and production
  geometry). Inspect Info/XMP: `GTS_PDFXVersion` and `pdfxid` say "PDF/X-4" with an
  output intent, while the JSON result says `independent_validation.status =
  "not-run"`. XMP title defaults to "Page 1".
- Expected: no PDF/X identification unless validation ran, or an explicit
  "unvalidated draft" marker; document title taken from the document name.
- Actual: file claims PDF/X-4. Content is a flattened raster, no vector or embedded
  fonts. `scripts/check-pdfx.py` returns ok with `conformance_claim:false`, which
  proves only structure.
- Suspected: `src/pdf.rs` (PDF/X metadata writer), `src/color/pdfx*`.
- Test: `tests/v0120_bundle.rs` or a new `tests/v0120_pdfx_label.rs` named
  `pdfx_output_reports_unvalidated_and_uses_document_title`.
- Acceptance: documented wording matches the file contents; XMP title equals the
  document name; release docs say "draft, unvalidated" wherever PDF/X is mentioned.
- Decision: patch-safe for the title and wording; dropping the identifiers would be a
  format decision (needs decision).

## PRINT-003  Soft proof, gamut and paper simulation have no CLI (P2, needs decision)

- Repro: `pentool --help | grep -i proof` finds none; only `/api/color/proof`,
  `/gamut` work. README and roadmap CLI direction mention `pentool proof`.
- Expected: reachable by agents offline from the CLI, as AGENTS.md requires
  deterministic CLI operations.
- Actual: server-only.
- Files: `src/main.rs`, `src/color/*` proof functions.
- Test: `tests/v0120_print_ui.rs` or new `tests/v0120_proof_cli.rs`
  `color_proof_cli_matches_endpoint_bytes`.
- Acceptance: either add a thin `color proof`/`color gamut` wrapper over the shared
  function, or mark the roadmap direction as "server only" and tick nothing.
- Decision: adding commands is a feature; maintainer decides. Doc correction is
  patch-safe.

## PRINT-004  No CLI to add or edit spot inks (P2, needs decision)

- Repro: spot plates only appear after hand-writing
  `{"schema":1,"inks":{...}}` into `color_management.inks` of the `.pen`.
- Expected: item 8 "offline ink libraries" manageable without editing JSON.
- Files: `src/color/ink*`, `src/main.rs`.
- Test: `tests/v0120_separation.rs` `ink_library_round_trips_through_cli`.
- Acceptance: documented procedure (README/color doc) for adding inks, or a command.
- Decision: documenting is patch-safe; a command is a feature.

## PRINT-005  Preflight says ok where PDF/X and package hard-fail (P2, patch-safe)

- Repro: document with a spot-ink paint and overprint. `color preflight DOC` ->
  `ok:true`, warnings `spot-not-in-pdfx`, `overprint-not-in-pdfx`. Then
  `color pdfx DOC out.pdf` and `color package DOC --out dir` -> `[unsupported-capability]
  ... uses spot-ink paints`.
- Expected: preflight with the same target reports the blocker as an error, or the
  warning text says export will refuse.
- Files: `src/color/preflight.rs` (severity for `spot-not-in-pdfx`), PDF/X exporter.
- Test: `tests/v0120_bundle.rs` `preflight_flags_what_pdfx_export_refuses`.
- Acceptance: preflight `ok` is false (or an `export_blockers` field is non-empty)
  exactly when `color pdfx` would refuse; dry-run/commit agreement preserved.

## PRINT-006  stroke-field README example and caps (P3, patch-safe)

- Repro: README `raster grass.pen stroke-field paint --field '{"method":"radial",
  "center":[128,128],"cell":8}' ...` on a 320x240 layer -> `[limit-exceeded] more
  than 512 strokes`. Works at 256x256. `--dry-run` after the subcommand is a clap
  error; it must precede it.
- Expected: README states layer size and the 512-stroke cap and shows
  `raster FILE --dry-run stroke-field`; protocol template caps agree.
- Files: `README.md` (~line 1268), `docs/agent-painting-protocol-v1.md`.
- Test: `tests/v0130_protocol.rs` `readme_stroke_field_example_runs` (extract the
  documented command, run on a 256x256 layer).
- Acceptance: doc text matches the implemented cap; example runnable verbatim.

## PRINT-007  Normative plan fixture is not a runnable plan (P3, patch-safe)

- Repro: `pentool paint-plan doc.pen docs/fixtures/agent-painting-v1-valid-plan.json
  --dry-run` -> `[invalid-request] unknown plan limit "max_generated_strokes"`.
  The fixture also has no `action` per pass; it is the engine-side schema form.
- Expected: fixture name or a README note says "schema example, not authoring form",
  or limits are aligned.
- Files: `docs/fixtures/agent-painting-v1-*.json`, `docs/agent-painting-protocol-v1.md`.
- Test: `tests/v0130_protocol.rs` `fixture_plan_is_documented_as_schema_form`.
- Acceptance: a reader cannot mistake the fixture for an authoring plan.

## PRINT-008  Palette extraction dominated by near-transparent pixels (P3, patch-safe)

- Repro: `paint-palette portrait.pen extract` role "shadow" on a mostly transparent
  layer -> `#80404004` at 0.91 share.
- Expected: alpha-weighted or alpha-thresholded extraction, or documented behavior.
- Files: `src/paint_palette.rs` (extract).
- Test: `tests/v0130_palette.rs` `extract_ignores_near_transparent_pixels`.
- Acceptance: with the same input the dominant swatch has alpha above the threshold,
  outputs stay seeded/deterministic.

## PRINT-009  Stale roadmap and doc text (P3, patch-safe)

- `docs/professional-color-v1.md` ~line 796 says trapping, photo-catalog separations
  and composite plate preview "remain open" (they are implemented).
- v0.12 TASKS item 14 says spot-to-process substitution is open; DEFERRED says not
  implemented; only spot-to-spot works. Make the three agree.
- `docs/roadmap/v0.14.0/REMAINING.md` lists 12, 15, 16, 17, 18 as "Not started" but
  `layout-merge/adapt/alt`, `layout-export`, `layout-preflight` and the Studio layout
  panel exist.
- Test: none (docs). Acceptance: text matches the audit rows.

## PRINT-010  Help gaps (P3, patch-safe)

- Print recipe JSON requires `version:1` and `name`; help/README do not say so.
  README `--recipe magazine-print.json` shows no recipe body.
- `inspect ink` requires `--tac-limit`, `--small-text-points`, `--units-per-inch` with
  no defaults and no note in `--help` summary.
- Test: `tests/v0120_marks.rs` or the CLI help test: `recipe_help_mentions_version_and_name`.
- Acceptance: help and README include a minimal valid recipe and ink-analysis example.

## PRINT-011  Normative layout fixture cannot be rendered (P2, patch-safe)

- Repro: `cp docs/fixtures/layout-v9-valid.pen v.pen; pentool export v.pen o.png` ->
  `[malformed-resource] v5 image_assets must be an object`; `layout-export v.pen o.pdf`
  -> `[render-failed] page page-1: [malformed-resource] v5 image_assets must be an
  object`. Adding `image_assets` then fails `[unsupported-capability] v6 requires
  compositing engine 1 and srgb8`.
- Expected: a v9 document without those optional blocks either renders (defaults) or
  the validator rejects it as incomplete, rather than the layout composer and
  `layout-preflight` accepting it and export failing.
- Files: `src/image.rs` (lines ~94, 1298, 2000), `src/main.rs` export path, fixture.
- Test: `tests/v0140_layout_spec.rs` `valid_v9_fixture_exports_png_and_pdf`.
- Acceptance: the fixture exports, or is completed and the spec states the required
  top-level blocks. Check migration/validation agree with `docs/layout-v1.md`.

## PRINT-012  `book export` README example needs an output role (P3, patch-safe)

- Repro: README `pentool book guide.penbook export guide.pdf --ppi 300` on members
  without a color role -> `[missing-resource] output color role is not configured`.
- Expected: README says the print export needs `color role assign` first.
- Files: README (~line 423), `docs/layout-v1.md`.
- Acceptance: example text states the prerequisite.

## PRINT-013  `.indd` refusal message (P3, patch-safe)

- Repro: `pentool idml-import file.indd out.pen` -> `[malformed-resource] not a ZIP
  archive`. The help promises native `.indd/.indb` are refused.
- Expected: extension-based `[unsupported-format]` message naming IDML export as the
  path.
- Files: `src/idml*.rs` import entry. Test: `tests/v0140_idml.rs`
  `indd_extension_is_refused_with_guidance`.
- Acceptance: error names InDesign native format and suggests exporting IDML.

## Checkbox changes (evidence in audit-print.md)

v0.12.0 TASKS: leave all 16 unticked (consistent with DEFERRED.md; cross-OS, RIP and
validator evidence missing). Items 1-5, 9, 10 are substantially implemented and may
be ticked only after a maintainer accepts "no reference-RIP/validator" as in scope.
Do not tick 11, 12, 15, 16.

v0.13.0 TASKS: all 14 ticked. Keep ticks for 1, 2, 4, 5, 7-11. Consider unticking or
caveating 3 (PRINT-008), 6 (PRINT-006), 12 (package/component/batch not run here)
and 13 (UI unverified). 14 depends on CI evidence I could not check.

v0.14.0 TASKS: all unticked. Evidence supports ticking nothing on its own because
the doc lists known limits (estimated composition, whole-document recompose,
incomplete goldens/fuzz). Items 2, 6, 14, 15, 16 behaved fully as promised in my
runs and are candidates for ticking once their dedicated tests are confirmed green
in CI.

## Overstated or misleading claims

1. PDF/X-4 identification in output files without validation (PRINT-002). No
   "conformance" should be claimed; the tool is honest in JSON but the file is not.
2. v0.12 item 15 print UX in docs versus Studio reality (PRINT-001).
3. Roadmap `pentool proof/preflight/export --recipe` direction: commands do not exist
   under those names (PRINT-003).
4. `layout-v9-valid.pen` is called valid but cannot be exported (PRINT-011).
5. v0.13 item 14 "CI green" and cross-platform digest claims cannot be verified
   locally.

## Not covered / UNVERIFIED

Browser behavior of Studio print, paint review and layout panels; PDF/X conformance
under veraPDF or Acrobat; EPUBCheck; hostile ICC fuzz; macOS/Linux results; paint
packages/components/batch; v0.14 `wrap set`, anchors, TOC/index, `frame-grid`,
`step-repeat`, `layout-adapt`, table place/merge; performance figures.
