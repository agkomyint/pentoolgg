# Stabilization tasks: raster (v0.7.0 - v0.11.0)

Source: `audit-raster.md`. Repro dir: session scratchpad `audit-raster/` (t7..t11). "Patch-safe" = fix only,
no format or CLI contract change. Default test home for new regression tests: `tests/stab_raster.rs`
unless a better owning file is named.

## RASTER-001 documented gradient stop key `offset` is rejected
- **Status: fixed in 0.20.1.** Reproduced. Released behavior and tests use `position`, so per the AGENTS.md precedence the docs and the gradient-overlay hint were corrected to `position`, and a stop with `offset` but no `position` now fails with an error naming the right key (fill, gradient-map, gradient-overlay). No alias added (that would be a new input spelling). Tests: unit `composite::gradient_stop_tests`, `fill::gradient_stop_tests`.
- Severity: P1. Patch-safe if `offset` is accepted as an alias; docs-only fix needs a maintainer decision (alias is safest).
- Repro:
  - `pentool new c.pen --width 300 --height 200`
  - `pentool fill add c.pen g1 --kind linear --params '{"stops":[{"offset":0,"color":"#ff0000"},{"offset":1,"color":"#0000ff"}]}' --layer layer-1 --x 10 --y 130 --width 200 --height 50`
  - same `stops` with `adjustment add ... gradient-map` and `effect add ... gradient-overlay`.
- Expected: succeeds as `docs/params-reference.md` (lines ~15, ~38) and the `gradient-overlay` error hint describe. Actual: "gradient position must be in 0..1" / `[invalid-adjustment] gradient position must be in 0..=1`; only `"position"` works (misleading, the value is in range).
- Suspects: `src/composite.rs` `gradient()` (~515-530), `src/fill.rs` (~113), the effect gradient-overlay hint string, `docs/params-reference.md`.
- Test: `tests/v081_fixes.rs` `gradient_stops_accept_offset_alias` (fill, gradient-map, gradient-overlay), plus `gradient_stop_error_names_missing_key`.
- Accept: `offset` and `position` both render identical pixels; an out-of-range or missing key yields an error naming the key actually missing; docs and hint list the accepted spelling(s).

## RASTER-002 image assets cannot be created or indexed as asset documents
- Severity: P2. Needs a maintainer decision (support v5 image assets in `asset create/inspect/library`, or document that packages are the only route).
- Repro: `pentool image add d.pen im1 ...` then `pentool asset create d.pen ...` -> "could not flatten v4 scene... v3 flattening cannot represent image node". Install a package containing an image asset: `.pentool/index.json` shows `invalid_asset` warnings, 0 assets; `pentool asset inspect` -> "document has no canvas".
- Expected: image assets listed/inspectable/addable, or a clear unsupported message at creation. Actual: no CLI route creates a packageable image asset (tests build the JSON by hand, see `tests/v070_hardening.rs` ~529) and installed ones are invisible to `explore`/`add`.
- Suspects: `src/asset.rs` (v3 flatten in create/inspect), `src/library.rs` index scan, `src/package.rs`.
- Test: `tests/v070_hardening.rs` `installed_image_asset_is_indexed_and_inspectable`.
- Accept: `asset inspect` and library index succeed for v5 image-asset docs (or the earlier failures become one documented `[unsupported-capability]` naming the alternative).

## RASTER-003 UNVERIFIED: raster batch exited nonzero with empty stderr once
- Severity: P2 until reproduced (then P1 if data is affected). Patch-safe.
- Repro attempt: 40 sequential `pentool raster r.pen batch bN.json` (250 `stroke` ops each, seeded random positions, `--brush {"size":6}`) on a shared Windows machine; call N=35 exited nonzero with no stderr captured, the identical file re-ran successfully (exit 101 was only a closed pipe from `head`). Final state verified (`verify --replay` match, `journal_dropped 9987`), so no corruption was seen.
- Expected: no spurious failure; any failure prints a coded error. Suspect: history/lock contention with other processes (antivirus, other auditors) mapped to a silent exit; `src/transaction.rs` retry/rename path.
- Test: `tests/raster_batch.rs` `repeated_batches_never_fail_silently` (loop 40, assert every failure has stderr).
- Accept: reproduce or document as environmental; any nonzero exit prints an `Error:` line.

## RASTER-004 documentation drift (cluster)
- Severity: P3. Patch-safe (docs only).
- Items:
  - `docs/params-reference.md` lists 4 selection kinds; code supports rectangle, ellipse, polygon, lasso, color-range, luminosity-range, node-alpha, mask-alpha, and add/subtract/intersect (`kind` + `a`/`b`; `{"op","left","right"}` fails with "selection kind missing").
  - Batch composite ops (`fill-add`, `adjustment-add`, `effect-add`, `clip-add`; clip needs top-level `base`) are documented only in `tests/v080_portability_batch.rs`.
  - `effect list FILE ID EFFECT_ID` documented but errors "effect list takes only FILE and NODE_ID".
  - `effect add --kind blur --blur 1` rejected; the parameter is `--params '{"radius":1}'`.
  - v0.7.0 item 17 says 24,000,000 B budget; CI and release gate at 60,000,000 B (installed binary 55.7 MB).
  - `PENTOOL_AI_ENDPOINT` only takes effect with `--ephemeral` or when it selects the provider; with a saved config it silently resolves to the saved `gemini` profile.
  - v0.10.0 CLI examples (`pentool brush`, `paint stroke`, `clone`, `heal`, `select wand`) differ from the shipped `pentool raster FILE <sub> ID`; roadmap says provisional but README should show real forms.
  - `--dry-run`/`--if-revision` on `raster` go before the subcommand; after it clap says "unexpected argument".
- Test: `tests/doc_examples.rs` (or existing README example runner) - run each documented example.
- Accept: each documented command line runs as written; budget text matches the gate.

## RASTER-005 transform ergonomics
- Severity: P3. Needs a maintainer decision for a `transform list` subcommand; positional id fix is patch-safe.
- Repro: `pentool transform FILE NODE enable OP_ID` -> "transform settings apply only to add/set" (positional parsed as KIND); `pentool transform FILE NODE list` -> "unknown transform stack operation".
- Suspects: `src/main.rs` ~9812-9830, `src/transform.rs` ~270-335.
- Test: `tests/v080_transform.rs` `transform_enable_accepts_positional_id_or_explains`.
- Accept: error names `--op-id`, or positional id works; `list` exists or help says use `inspect`.

## RASTER-006 blend modes against the canvas background
- Severity: P3. Needs a maintainer decision.
- Repro: node with `multiply`/`screen`/`difference` over only the page background renders identical to `normal`; over a backdrop rect results are correct (multiply 128,64,64).
- Expected: documented either way. Suspect: `src/composite.rs` backdrop handling; `COMPOSITING-SPEC.md` excludes page background for adjustments only.
- Test: `tests/v080_blend.rs` `blend_over_page_background_is_documented_behavior`.
- Accept: spec states behavior; test pins it.

## RASTER-007 minor image/mask noise
- Severity: P3. Patch-safe.
- (a) `pentool image op move --index 0` for an op already at 0 reports `changed:true` with equal bytes; should be a no-op `changed:false` (AGENTS.md: avoid successful no-ops that look like changes).
- (b) Serialized masks contain `-0.0` anchors; normalize to `0.0` for stable diffs.
- Tests: `tests/v070_hardening.rs` `op_move_to_same_index_reports_unchanged`; `tests/v080_masks.rs` `mask_json_has_no_negative_zero`.

## RASTER-008 raster CLI: spec-vs-ship naming and flag placement (docs/help)
- Severity: P3. Merge with RASTER-004 if preferred. Note only: `raster batch` caps at 256 ops per call, so "10,000 strokes" needs 40 calls; documented in help but not in the v0.10 acceptance text.
- Accept: acceptance text for 10,000 strokes names the batch cap, or the cap is raised.

## RASTER-009 missing referenced test for v0.7.0 item 14
- Severity: P3. Patch-safe.
- `docs/roadmap/v0.7.0/TASKS.md` cites `tests/web/image-panel.test.mjs`; no `tests/web/` exists and Classic is retired. Studio has `web-ui/src/workspaces/ImageAdjust.tsx`, `OperationsPanel.tsx` but only `api.test.ts`/`model.test.ts` as tests.
- Test: `web-ui/src/lib/api.test.ts` add `image_bake_and_add_routes` (or a vitest for OperationsPanel).
- Accept: roadmap cites a test that exists.

## Roadmap checkbox recommendations

Tick (implemented and verified here; v0.8.0 file, all currently unticked):
- 2 masks, 3 clipping, 8 selections, 9 inspection, 10 linked assets, 11 presets, 12 batch/diff. Evidence in `audit-raster.md`.
- Items 1, 5, 6, 7 only after RASTER-001 (and RASTER-005/mesh-warp deferral noted); 4 after RASTER-006 documents behavior.
- Leave 13 (editor parity, UNVERIFIED) and 14 (cross-platform goldens) open until evidence exists.

Leave unticked (v0.9.0): all, per header. Annotate items 5 and 9 as shipped ("generate, candidate review, keyout, remove-background"), items 1, 2, 3, 4, 6, 11, 13 as partially shipped, items 7, 8, 10, 12 as deferred.

Untick or annotate (v0.7.0): item 14 (test file cited does not exist; UI unverified), item 17 (budget 24 MB vs gate 60 MB), item 10 (no CLI to create image asset, RASTER-002).

v0.10.0 and v0.11.0: keep ticked; add "UI unverified" notes to v0.10 item 12 and v0.11 item 15, and "success path unexercised" to v0.11 items 5, 7, 8, 9 until a fixture-based test exists.
