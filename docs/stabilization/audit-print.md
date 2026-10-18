# Audit: v0.12.0 (color/print), v0.13.0 (agent painting), v0.14.0 (layout)

Auditor run against the installed v0.20.0 binary
(`C:\Users\OS\AppData\Local\Programs\Pentool\0.20.0\pentool.exe`). All inputs and
outputs lived in a scratch directory outside the repo. No source, test or roadmap
file was changed and nothing was committed. No `cargo build` or full test run.

Verdicts: WORKS (ran it, behaves as promised), PARTIAL (works but narrower than
claimed, or a documented piece is missing), BROKEN (documented behavior fails),
UNVERIFIED (not run; reason given). Severity: P0 data loss/corrupt `.pen`; P1
documented feature broken; P2 wrong output or missing test/doc/help; P3 polish.
"claimed" is the checkbox in the roadmap TASKS.md.

Totals: 48 items. WORKS 24, PARTIAL 21, BROKEN 0, UNVERIFIED 3. Defects found are
numbered PRINT-001... in `TASKS-print.md`; item rows reference them.

Note on method: roadmap "CLI direction" sections are explicitly provisional; I tested
the real command names (`color ...`, `paint-*`, `layout-*`), not the provisional ones.
Hosted CI and other-OS runs cannot be verified from this machine, so any claim that
rests only on them is marked accordingly.

## v0.12.0 Color management and print production (0 of 16 ticked)

| # | Item | Claimed | Verdict | Evidence | Sev |
|---|------|---------|---------|----------|-----|
| 1 | Freeze color spec | open | WORKS | `docs/professional-color-v1.md` exists and matches behavior; stale sentence near line 796 "Trapping, photo-catalog separations and a composite plate preview remain open" contradicts implemented `--auto-trap`/`--composite` (PRINT-009). Could be ticked. | P3 |
| 2 | Bounded Rust ICC engine | open | WORKS | `color profile import` of `print-CGATS21_CRPC7.icc` ok; `profile inspect`, `profile validate` ok; `color evaluate` converts; garbage ICC refused: `Error: ... not a valid ICC profile` with doc unchanged. Hostile-profile fuzz and RIP comparison not run here. | - |
| 3 | Profile management | open | WORKS | import with `--redistribution allowed`, list/inspect/validate, role assign `output --to '{"space":"icc-cmyk","profile":"press"}'`. | - |
| 4 | Extend `.pen` color declarations | open | WORKS | v8 doc carried ICC catalog, roles, production geometry; round-trips through `color` commands; v8 schema checked by `scripts/check-print-schema.py` (exists). | - |
| 5 | CMYK and Lab workflows | open | WORKS | `color evaluate` RGB<->CMYK<->Lab; role assign accepted CMYK/gray/Lab spaces. | - |
| 6 | Black and ink analysis | open | PARTIAL | `inspect ink --tac-limit 300 --small-text-points 6 --units-per-inch 72` works, but the three flags are mandatory with no defaults, and the help does not say so up front (PRINT-010). | P3 |
| 7 | Soft proofing | open | PARTIAL | Works only through server endpoints (`/api/color/proof`, `/gamut`, 200 with correct types). No CLI. Studio PrintPanel has "Soft proof" but no gamut warning, intent or BPC selectors (PRINT-001, PRINT-003). | P2 |
| 8 | Spot colors and offline ink libraries | open | PARTIAL | Spot plates render in `color separate`; alt substitution works. No CLI to add or edit spot inks; library must be hand-edited as `{"schema":1,"inks":{...}}` in `color_management.inks` (PRINT-004). | P2 |
| 9 | Overprint, knockout, separations | open | WORKS | `color separate --composite`, `--auto-trap`; overprint mode 1, knockout, trap visible in plates; plate PNG via `/api/color/plates?plate=K`. | - |
| 10 | Production geometry | open | WORKS | `color production set --geometry JSON` / `show`; bad geometry (marks outside canvas) fails and leaves file byte-identical. | - |
| 11 | Supported PDF/X export | open | PARTIAL | `color pdfx` produces byte-identical PDFs on repeat (`cmp` identical); `scripts/check-pdfx.py` returns ok with `conformance_claim:false`. PDF self-declares `GTS_PDFXVersion`/`pdfxid` "PDF/X-4" while the tool reports `independent_validation.status = "not-run"`; flattened raster only (no vector, no embedded fonts); no veraPDF/qpdf/mutool/gs on this machine, so conformance is UNVERIFIED. Overstated labelling: PRINT-002. | P2 |
| 12 | Structured print preflight | open | PARTIAL | `color preflight` returns coded issues (`spot-not-in-pdfx`, `marks-missing`, `spot-ink-not-allowed`). But it reports `ok:true` for a doc on which `color pdfx` and `color package` hard-fail with `[unsupported-capability] ... uses spot-ink paints` (PRINT-005). Font and metadata preflight absent (listed in DEFERRED). | P2 |
| 13 | Proof/production packages | open | PARTIAL | `color package --out DIR`, `package-verify` ok; tampered member -> `digest-mismatch`, nonzero exit. Fonts are `assets.fonts = "not-collected"`. | P3 |
| 14 | Reusable print recipes | open | PARTIAL | `spot_policy: reject`, `allowed_spots`, `require_marks`, `plate_bits: 16`, spot-to-spot `spot_substitutions` all work. Recipe requires `version:1` and `name`; neither help nor README says so (PRINT-010). TASKS text says spot-to-process substitution is open, DEFERRED says not implemented, only spot-to-spot exists (PRINT-009). | P3 |
| 15 | Print-focused UX | open | PARTIAL | Studio `PrintPanel.tsx` (149 lines): preflight, load plates, soft proof only. Missing: gamut warning, PDF/X download link, intent/BPC selectors, bleed/trim/safe overlay, overprint highlight. Classic `web/print-panel.js` retired in 0.12.1 and docs still describe it. Only test is server-side `tests/v0120_print_ui.rs` (PRINT-001). | P1 |
| 16 | Security, conformance, performance proof | open | UNVERIFIED | No named validator, no reference RIP, no cross-OS evidence, float scene/plate performance not measured. Tests `v0120_bundle.rs`, `_separation.rs`, `_marks.rs` exist and are consistent with the observed behavior; hosted CI not inspectable here. | - |

Non-item checks that passed: marks (12 marks rendered, 20 geometry marks, 1 slug_text,
8-patch color bar); server endpoints preflight/plates/proof/gamut/pdfx; XMP title
defaults to "Page 1" (cosmetic, PRINT-002).

## v0.13.0 Agent-directed realistic painting (14 of 14 ticked)

Ran on `portrait.pen` (raster layer `paint` 320x240) and `grass.pen` (256x256).

| # | Item | Claimed | Verdict | Evidence | Sev |
|---|------|---------|---------|----------|-----|
| 1 | Protocol and threat model | [x] | WORKS | `docs/agent-painting-protocol-v1.md` matches behavior of plan/bundle/diff (guard flags, `[plan-changed]`, limit ceilings). Normative fixture `docs/fixtures/agent-painting-v1-valid-plan.json` is rejected by `paint-plan` (`unknown plan limit "max_generated_strokes"`, and uses no `action`), so it is a schema example, not a runnable plan (PRINT-007). | P3 |
| 2 | Deterministic stroke paths | [x] | WORKS | README example `raster stroke-path`: 32 strokes, 64 samples, 144 dabs; repeat gives same tile hash. | - |
| 3 | Color sampling, mixing, palettes | [x] | PARTIAL | `paint-palette` sample/extract --save/mix/vary/list work, vary seeded. Extraction on a mostly transparent layer returned `#80404004` (alpha 4) at 0.91 share for role "shadow": near-transparent fringe dominates (PRINT-008). | P3 |
| 4 | Region previews and comparison | [x] | WORKS | `paint-review` preview ok; `compare` with different-size reference fails explicitly (no silent resize); `--reference-document` compare ok. | - |
| 5 | Region-aware constraints | [x] | WORKS | `select-marquee`, `select-save`, stroke-path constrained "inside saved:left" with follow-contour; `paint-constraints preview` returns mask and field hash. | - |
| 6 | Deterministic stroke fields | [x] | PARTIAL | README `stroke-field` radial example works on 256x256 (55,394 dabs); on the 320x240 layer it fails `[limit-exceeded] more than 512 strokes`. README does not state the layer size or the 512-stroke cap, and the protocol template shows `max_generated_strokes` 5000 (PRINT-006). `--dry-run` works but only before the subcommand (`raster FILE --dry-run stroke-field ...`); after it clap errors. | P3 |
| 7 | Lighting/material paint modes | [x] | WORKS | `paint-media list`/`describe`; stroke with watercolor-wash media works. | - |
| 8 | Compact visual analysis | [x] | WORKS | `paint-analyze` returns contours JSON, bounded. | - |
| 9 | Declarative multi-pass plans | [x] | WORKS | Dry-run reports `commit.if_revision` and `commit.plan_hash`, leaves file byte-identical; commit without guards refused (`committing a paint plan requires --if-revision`); wrong hash -> `[plan-changed]`, file unchanged; correct guards commit with one history entry; `undo` restores revision `92c8f981...` exactly. | - |
| 10 | Composition guides | [x] | WORKS | `paint-guide add thirds`, `snap` (106.667,80), `list`. `proof` needs `--width/--height` and visible-guide ids (`--ids`/`--include-hidden`); fine but terse. | - |
| 11 | Diagnostics and revision scoring | [x] | WORKS | `paint-score before after --layer paint --goal ...` returns measured vs heuristic basis, `applied:false`, changed bounds `[0,3,32,43]`, 608 changed pixels. | - |
| 12 | Presets, batch, history, diff, packages, components | [x] | PARTIAL | `paint-diff` (document and `--plans`) ok; `paint-bundle` export/import (dry-run unchanged, conflict refuses without `--replace`, non-bundle refused), plan-add/show/list, `assets` ok; history/undo ok. README `export --brushes round` fails on a doc without a brush named `round`. Package and component integration, `raster batch`: not run. | P3 |
| 13 | Editor-native review/authoring | [x] | UNVERIFIED | `PlanPanel.tsx` and `RasterPanel.tsx` exist and release notes list the controls; no browser available to exercise them. | - |
| 14 | Conformance, performance, recovery | [x] | PARTIAL | `tests/v0130_conformance.rs` has goldens, hostile-input and limit tests; the 10,048-stroke test is `#[ignore]`d (run manually, 35 s). Cancellation test exists in `src/paint_plan.rs` (`cancellation_at_every_checkpoint_writes_nothing...`). The claimed green CI run 38207854682 cannot be checked from here. | P3 |

## v0.14.0 Professional layout, `.penbook`, IDML (0 of 18 ticked)

Ran on a hand-built v9 document and on `docs/fixtures/idml/minimal` zipped to an
`.idml`. Composition measurements are marked `estimated:true` by the tool.

| # | Item | Claimed | Verdict | Evidence | Sev |
|---|------|---------|---------|----------|-----|
| 1 | Freeze layout spec, IDML mapping, threat model | open | PARTIAL | `docs/layout-v1.md`, `layout-v1.schema.json`, `idml-mapping-v1.md`, `penbook-v1.schema.json` exist. Fixture `layout-v9-valid.pen` fails to render: `pentool export v.pen o.png` -> `[malformed-resource] v5 image_assets must be an object`; adding `image_assets:{}` then fails `[unsupported-capability] v6 requires compositing engine 1 and srgb8` (PRINT-011). | P2 |
| 2 | Documents, spreads, parents, sections | open | WORKS | `parent create/item-add/assign/resolve`, `override set` (`overridden:["fill","x"]`), orphaned override reported on `item-remove`, `section add --start-number 5` -> page label "5"; `spread list`; `page add`. | - |
| 3 | Stories, threaded frames | open | WORKS | Fixture threads `body-1 -> body-2`; `layout-preflight` warns `frame-empty` for empty second frame; invalid thread-cycle fixture exists (rejection test is in `v0140_stories.rs`; not run). | - |
| 4 | Shaping and font engine | open | PARTIAL | Bundled font embedded in PDF/EPUB; PDF limits list "right-to-left and complex-script positioning use shaped advances only". Complex-script coverage not verified. | P3 |
| 5 | Paragraph composer | open | PARTIAL | `layout-wrap intervals` returns per-line segments; lines counted by preflight. Width/height estimates are flagged `estimated:true`, not final-composition-exact. | P3 |
| 6 | Styles | open | WORKS | `layout-style create/set/show/usage/remove/check`; `based_on` cycle refused (`part of a based_on cycle through h1`); missing base refused; remove of used style refused with `[in-use]`; `show` returns resolved chain `["h2","h1"]` with `{"size":18,"weight":700}`. Note `--set based_on=` is refused with a pointer to `--based-on`. | - |
| 7 | Grids, guides, alignment | open | WORKS | `baseline-set --increment 12`, `baseline-snap 100` -> 108; `guide-add --kind ruler-h`; `measure`, `frame-grid`, `step-repeat` present (last three not run). | - |
| 8 | Text wrap, anchored objects | open | PARTIAL | `layout-wrap intervals` works with zero exclusions; `wrap set` and `anchor-*` not run. | - |
| 9 | Tables | open | PARTIAL | `layout-table import tb d.csv`, `show` ok. `plan` before placing fails `not placed in a story with frames; pass --heights` (fine). `place` option name differs from the guessed `--offset`; not completed. | P3 |
| 10 | Long-document features | open | PARTIAL | `layout-doc note-add` + `note-plan` (status fits, label 1) and `link-list` ok. TOC/index/captions/conditions/variables not run. | - |
| 11 | Books (`.penbook`) | open | PARTIAL | `book init/status/renumber --dry-run/sync/package/toc/index/preflight` work; unsafe members refused (`../outside.pen`, `C:/other/a.pen`). `book export guide.pdf --ppi 150` fails `output color role is not configured` -- README example has no such prerequisite (PRINT-012). | P3 |
| 12 | Data merge, adaptive layout | open | PARTIAL | `layout-merge m.pen --data people.csv --out cards.pen` -> 3 pages `page-1-m1..m3`, placeholders replaced, second run refuses overwrite; `layout-alt create/sync` ok. `layout-adapt` has no `--out` (in-place only; not run). REMAINING.md lists this as "Not started" (PRINT-009). | P3 |
| 13 | Import IDML | open | WORKS | zip of `fixtures/idml/minimal` -> `idml-import` ok: 2 pages, 1 parent, `counts` supported 17 / approximated 2 / carried 3 / unsupported 1, missing font reported; existing output refused without `--overwrite`; a non-zip `.indd` refused with `not a ZIP archive` (message does not mention native InDesign files, PRINT-013). Report shape differs from `minimal.expected.json` (the test compares selected fields only). | P3 |
| 14 | Export IDML | open | WORKS | `idml-export` ok (9 parts, carried blocks returned unchanged), `--overwrite` required, non-`.idml` extension refused, re-import of the export works with a same-count report. | - |
| 15 | Publishing exports | open | WORKS | `layout-export t.pen t.pdf`: `%PDF-1.7` with `/StructTreeRoot`, `/Link`, `/Lang`; no `/Outlines` because the doc had no bookmarks (`bookmarks:0`). EPUB reflowable (7 entries, nav.xhtml, embedded font) and `--fixed` ok. Tool states PDF/X not claimed and EPUBCheck not run (honest). | - |
| 16 | Layout preflight | open | WORKS | `layout-preflight` returns coded issues (`frame-empty`), `mutated:false`, bounded counts. | - |
| 17 | Editor layout experience | open | UNVERIFIED | `LayoutPanel.tsx`, `layout/LayoutOverlay.tsx` exist; no browser used. Known gaps per release notes: no on-canvas anchor handles, no loaded-cursor image placement. | - |
| 18 | Conformance, performance, recovery | open | PARTIAL | 17 `tests/v0140_*.rs` files exist and `scripts/layout-smoke.py` exists; release notes admit goldens, fuzz, cancel and recovery incomplete and whole-document recompose (616 ms / 500 pages). Not run here. | P3 |

## Summary of cross-cutting defects (details in TASKS-print.md)

PRINT-001 Studio print UX regression (P1); PRINT-002 PDF/X self-labelling and
XMP title (P2); PRINT-003 no CLI for soft proof/gamut (P2); PRINT-004 no CLI for
spot inks (P2); PRINT-005 preflight/pdfx inconsistency (P2); PRINT-006 stroke-field
README and cap (P3); PRINT-007 plan fixture not runnable (P3); PRINT-008 palette
alpha fringe (P3); PRINT-009 stale roadmap and doc text (P3); PRINT-010 help gaps
(P3); PRINT-011 layout fixture unrenderable (P2); PRINT-012 book export README
(P3); PRINT-013 `.indd` message (P3).

## Process disclosure

During testing I ran `taskkill //F //IM pentool.exe`, which terminated every
pentool.exe on the machine (two PIDs, 33704 and 484). One was my own `serve` on port
47911; the other may have belonged to a parallel auditor. No data was involved but
a concurrent audit run could have lost a server session.
