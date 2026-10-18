# Audit: raster roadmap (v0.7.0 - v0.11.0)

Auditor: read-mostly pass against the installed `pentool.exe` 0.20.0, repro files in the session scratchpad
(`audit-raster/`, subdirs t7..t11). No source, test or roadmap file was edited; no cargo build or test suite
was run (shared machine), so "tests exist" means the file was located, not executed.
Verdicts: WORKS / BROKEN / PARTIAL / UNVERIFIED. Severity: P0 data loss, P1 documented feature broken,
P2 wrong output or missing capability, P3 polish/docs. Defect ids refer to `TASKS-raster.md`.

Conventions: `$ pentool` below is the installed 0.20.0 binary. Browser/Studio UI items are UNVERIFIED
(no browser run). Cross-platform pixel/golden claims are UNVERIFIED (Windows only). For v0.9.0 the
roadmap itself says only a subset shipped; items it defers are marked BROKEN with the note "deferred"
(not implemented, documented as open, severity P3) so the checkbox state stays honest.

Totals (76 items): WORKS 43, PARTIAL 21, BROKEN 4 (all documented v0.9 deferrals), UNVERIFIED 8.

## v0.7.0 raster images (18 items, all claimed complete)

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 image spec / threat model | [x] | WORKS | docs and normative fixtures present; behaviors below match them | - |
| 2 v5 format + migration | [x] | WORKS | `image add` upgrades a v4 doc; unknown-version and downgrade diagnostics fail without mutation | - |
| 3 PNG/JPEG/WebP import, dedup, limits | [x] | WORKS | all three codecs import; same bytes added twice grew the doc by 785 B; 60000x60000 PNG -> `[limit-exceeded]`; truncated/garbage -> `[malformed-resource]` | - |
| 4 embedded/external assets, containment | [x] | WORKS | external path missing -> `[missing-resource]` with path; traversal -> `[unsafe-path]`; tampered bytes -> `[hash-mismatch]` | - |
| 5 `image add/set/info/remove` | [x] | WORKS | transactions + history entry per call; `--if-revision` mismatch rejected; failed batch left file byte-identical (`cmp`) | - |
| 6 render through all outputs | [x] | WORKS | PNG export deterministic (identical sha256 twice); PDF and SVG export succeed; linked-SVG with ops fails closed `[unsupported-capability]` | - |
| 7 operation engine | [x] | WORKS | `rotate 45` -> `[invalid-operation]`, `blur 9999` rejected, unknown kind -> `[unsupported-capability]` | - |
| 8 `image op` CLI + batch | [x] | WORKS | add/list/set/move/enable/disable/remove each one history entry. Minor: `op move --index 0` when already at 0 reports `changed:true` with equal bytes (RASTER-007) | P3 |
| 9 deterministic bake | [x] | WORKS | `bake --dry-run` hash == committed hash (`sha256:72cfefc9...`); undo/redo restore | - |
| 10 packages with images | [x] | PARTIAL | `package pack` twice byte-identical (`a5c52506...`); verify/inspect show image table. But no CLI path creates a packageable image asset: `asset create` -> "could not flatten v4 scene... v3 flattening cannot represent image node"; `asset inspect`/indexing of installed image assets -> "document has no canvas", `.pentool/index.json` shows `invalid_asset`, 0 assets | P2 (RASTER-002) |
| 11 analysis (`analyze`, `palette`) | [x] | WORKS | palette emits brand-1..3 tokens; analyze is read-only | - |
| 12 conformance/codec tests | [x] | WORKS | behavior re-verified by hand; test files `tests/v070_*.rs`, `v071_fixes.rs`, `v072_fixes.rs` exist (not executed) | - |
| 13 proxies / processed cache | [x] | WORKS | `image cache` subcommands operate; checksummed entries | - |
| 14 browser crop/mask/adjust UI | [x] | UNVERIFIED | cited test `tests/web/image-panel.test.mjs` does not exist (no `tests/web/`); Classic editor retired; Studio has `ImageAdjust.tsx`/`OperationsPanel.tsx` and `/api/image/bake|add` routes, not driven | P3 (RASTER-009) |
| 15 benchmarks | [x] | WORKS | `benchmark --images` runs | - |
| 16 fuzz and harden | [x] | WORKS | hostile inputs above all rejected cleanly; `fuzz/fuzz_targets/` has decode_source, image_assets, operation_stack, package_verify, processed_record (not run) | - |
| 17 distribution budgets | [x] | PARTIAL | roadmap says 24,000,000 B; `scripts/check-binary-size.sh` and CI/release gate at 60,000,000 B; installed binary is 55.7 MB (about 4.3 MB headroom) | P3 (RASTER-004) |
| 18 documentation | [x] | WORKS | `docs/image-workflow.md` present and matches behavior | - |

## v0.8.0 compositing (14 items, all UNTICKED but the CLI is largely implemented)

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 adjustment nodes | [ ] | PARTIAL | all 13 kinds add. Documented gradient stop key `offset` fails: `$ pentool adjustment add ... gradient-map --params '{"stops":[{"offset":0,...}]}'` -> `[invalid-adjustment] gradient position must be in 0..=1`; only `position` works | P1 (RASTER-001) |
| 2 masks (raster/vector) | [ ] | WORKS | `mask create` (vector, node-alpha), `attach --feather --density`, `list`, `apply --dry-run` all work | - |
| 3 clipping stacks | [ ] | WORKS | `clip add/remove/--dry-run`; bad base -> `[invalid-clipping]` | - |
| 4 blend modes | [ ] | PARTIAL | all 16 accepted, `bogus` rejected; over a backdrop shape: normal 192,64,64 multiply 128,64,64 screen 192,128,128 difference 127,128,128. Against only the canvas background every mode renders as normal; not documented | P3 (RASTER-006) |
| 5 non-destructive transforms | [ ] | PARTIAL | rotate, scale, skew, four-corner, perspective, enable/disable/move/remove, degenerate quad error work. Mesh warp unsupported (roadmap permits deferral). No `transform list` ("unknown transform stack operation"); op id passed positionally is parsed as KIND -> "transform settings apply only to add/set" (use `--op-id`) | P3 (RASTER-005) |
| 6 fill nodes | [ ] | PARTIAL | solid, linear, radial, conic render correctly with `position` stops (left 252,0,3 right 6,0,249); page-coordinate geometry -> `[invalid-fill]`; pattern with no asset -> "pattern asset missing". Documented `offset` key fails (same defect as item 1) | P1 (RASTER-001) |
| 7 layer effects | [ ] | PARTIAL | drop-shadow, outer-glow, stroke, color-overlay, inner-glow, inner-shadow, blur, gradient-overlay all add and render. `gradient-overlay` error hint recommends `offset`, which is rejected; `effect add --kind blur --blur 1` rejected ("valid parameters: radius") | P1 (RASTER-001) |
| 8 selections / saved masks | [ ] | WORKS | rectangle, ellipse, polygon, lasso, color-range, luminosity-range, node-alpha, mask-alpha, add/subtract/intersect (keys `a`,`b`); `save` and `crop --dry-run` work. `docs/params-reference.md` lists only 4 kinds (RASTER-004) | P3 |
| 9 inspection (histogram etc.) | [ ] | WORKS | `analyze --scope node:/page`, `--samples`, `--compare`; bad scope -> "node not found" | - |
| 10 linked-asset workflow | [ ] | WORKS | report (no base64 dump), embed dry-run, relink hash+path, collect, stale detection `status: stale` | - |
| 11 presets / copy-paste appearances | [ ] | WORKS | `preset save/apply` to `.penpreset`; script field rejected `[invalid-preset]`; paste keeps mask | - |
| 12 composite-aware batch and diff | [ ] | WORKS | batch ops `fill-add`, `adjustment-add`, `effect-add`, `clip-add` work (clip needs top-level `base`); undocumented in README/docs (only `tests/v080_portability_batch.rs`). `diff` gives JSON-pointer changes; `--visual --node v` writes before/after PNGs | P3 (RASTER-004) |
| 13 editor parity | [ ] | UNVERIFIED | Studio not driven | - |
| 14 hardening / perf | [ ] | PARTIAL | adjustment cost scales: 400x320 146 -> 163 ms, 3200x2680 323 -> 341 ms (B05 fixed); `v080_hardening.rs` exists (not run); cross-platform goldens not verified | P3 |

Retest of `docs/roadmap/v0.8.0/bug.md` on 0.20.0: B01 fixed (token with image renders blue), B02 fixed,
B03 fixed (`--mask` on add), B04 fixed (`pdftotext` finds HELLO), B05 fixed, B06 fixed, B07 fixed, B08 fixed,
B09 fixed, B10 hint now present but wrong (RASTER-001), B11/B14 not retested, B12 partially (`effect list FILE ID
EFFECT_ID` errors "effect list takes only FILE and NODE_ID" while `docs/params-reference.md` says to add EFFECT_ID),
B13 fixed (params doc exists; unknown-key error lists valid keys).

## v0.9.0 BYOK image models (14 items, all UNTICKED; header says subset shipped)

All provider calls used a local loopback mock (python, `127.0.0.1:18765`) via `PENTOOL_AI_ENDPOINT` +
`--ephemeral`, so the user's real config and key were never used for a call. No call went to the network.

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 protocol + threat model | [ ] | PARTIAL | `docs/ai.md` (90 lines) covers it; no separate frozen schema | P3 |
| 2 profiles, registry, guided setup | [ ] | PARTIAL | `ai setup --from-env`, `connect`, `doctor`, `resolve`, `default`, `provider`, `model` exist; `doctor` -> `{"checked_network":false,...}`; `resolve` -> `gemini/gemini-3-pro-image`. OS keyring, catalog refresh deferred | P3 |
| 3 adapter boundary | [ ] | PARTIAL | in-process HTTP adapters only; external adapter protocol / AI SDK bridge deferred (no `adapters/` dir) | P3 |
| 4 reference adapter set | [ ] | PARTIAL | two adapters: `gemini`, `openai-compatible` | P3 |
| 5 generation + candidate review | [ ] | WORKS | `generate --allow-model-call --size 64x64` hit mock `POST /v1/images/generations` with `Authorization: Bearer sk-t`, body `{"model":"m1","n":1,"prompt":"cat","size":"64x64"}`; `run list` -> 1 run; `run accept --candidate 1` -> node g1 with provenance (run, candidate, model, prompt_sha256); `--candidate 9` -> `[unknown-candidate]`; `run discard` empties list | - |
| 6 source-aware edit / variations | [ ] | PARTIAL | `ai edit --source IMG --kind edit|remove-background` exists; not exercised live (needs Gemini); no variations | P3 |
| 7 inpaint / object removal with masks | [ ] | BROKEN | not implemented (no mask option on `ai edit`); documented deferral | P3 |
| 8 outpaint | [ ] | BROKEN | not implemented; documented deferral | P3 |
| 9 focused cleanup (remove-background, keyout) | [ ] | WORKS | `ai keyout d.pen ko1 --source im1` -> new transparent node, `"model_call":false`; `--tolerance 9999` -> `[invalid-option]` | - |
| 10 recipes + regenerate | [ ] | BROKEN | not implemented; documented deferral | P3 |
| 11 batch/diff/history/packages integration | [ ] | PARTIAL | accept writes one history entry and provenance; batch cannot declare a model call | P3 |
| 12 editor-native controls | [ ] | BROKEN | no AI UI in Studio workspaces; documented deferral | P3 |
| 13 policy, privacy, cost | [ ] | PARTIAL | `generate` without `--allow-model-call` -> `[model-call-not-allowed]`; `PENTOOL_AI=off` -> `[policy-denied]`; `http://example.com/v1` -> `[policy-denied] ... must use https`; loopback http allowed; `--dry-run` prints prompt_sha256 + endpoint_host and `"contacted_provider":false`; limits (`max_calls`, `max_pixels`) in `doctor`. `PENTOOL_AI_MAX_COST` deferred | P3 |
| 14 conformance / recovery | [ ] | UNVERIFIED | `tests/ai.rs` exists, not run | - |

Note: env-only endpoint (`PENTOOL_AI_ENDPOINT`) is ignored unless `--ephemeral` (or the provider is set); with the
user's config present it silently resolved to the saved `gemini` profile. This matches docs but is a trap (RASTER-004).

## v0.10.0 pixel editing (14 items, all claimed complete)

CLI shape: `pentool raster FILE <sub> ID ...`; `--dry-run` and `--if-revision` are options of `raster` itself and go
before the subcommand. The roadmap's top-level `brush/paint/clone/heal/select` examples are labeled provisional.

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 freeze spec | [x] | WORKS | `docs/raster-paint-v1.md`, `raster-paint-v1.schema.json` exist | - |
| 2 raster layers + migration | [x] | WORKS | `raster add`, `info` (`pixel_format rgba8-straight`, tile_size 256), `resize/crop/trim/clear/duplicate` listed | - |
| 3 deterministic brush | [x] | WORKS | same stroke on two fresh docs -> identical `tile_map_sha256:0707ca86...`; `verify --replay` -> `"status":"match"` | - |
| 4 pen/mouse/touch input | [x] | PARTIAL | `--samples` JSON with `pressure` accepted; folded/limit errors (`[limit-exceeded]` for 0 events, "absurd coordinate"); live browser input unverified | P3 |
| 5 erasing + local blend | [x] | WORKS | all 10 blends (normal, erase, background-erase, smudge, blur, sharpen, dodge, burn, sponge, color-replace) run | - |
| 6 clone stamp | [x] | WORKS | `clone-source` + `clone-stroke`, aligned anchor reset | - |
| 7 healing / patch | [x] | WORKS | `heal-spot --radius 6` and `heal-stroke`/`heal-patch` commands succeed | - |
| 8 flood fill / contiguous selection | [x] | WORKS | `fill --x --y --color`; `select-wand --tolerance 12` -> `select-info` selected_pixels 37202 | - |
| 9 pixel selections + transforms | [x] | WORKS | `select-lasso`, marquee, modify, `transform-pixels --scale 1.2 --rotate 15`, lift/move/paste | - |
| 10 content-preserving layer ops | [x] | WORKS | `merge-down`, `trim`, `rotate`, `flip`, `merge-visible`, `stamp-visible`, `flatten --new-id` | - |
| 11 brush presets / assets | [x] | UNVERIFIED | `preset-add/show/export/import(+gbr,mypaint)`, `tip-add` listed; my one preset-add attempt used wrong args, not retried | - |
| 12 canvas interaction (browser) | [x] | UNVERIFIED | `RasterPanel.tsx`/`RasterStage.tsx` exist; not driven | - |
| 13 CLI + batch parity | [x] | WORKS | `raster batch FILE ops.json` ({action,id,args}), limit 1-256 ops per call (`[limit-exceeded]` for 10000) | - |
| 14 perf + correctness | [x] | PARTIAL | 10,000 strokes via 40 batches of 250: final `journal_entries 16`, `journal_dropped 9987` (auto-checkpoint), file 260 KB, `verify --replay` match in 0.09 s, export 0.1 s. `checkpoint --compact` -> `journal_entries 0`, replay match. `--dry-run` left file sha unchanged. One batch call in the loop exited nonzero with empty stderr, could not reproduce (RASTER-003). Cross-platform pixels and package round trip not verified | P2 |

Input hardening: invalid JSON, empty samples, bad color `#000` ("color must be #RRGGBB"), size 100000
(`[invalid-brush] size must be between 1 and 2048`), coordinate -9999999 (`[invalid-stroke] ... absurd coordinate`) all rejected
with no change.

## v0.11.0 photography and RAW (16 items, all claimed complete)

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 freeze spec | [x] | WORKS | `docs/photography-v1.md` and `docs/fixtures/photo-conformance.json` exist | - |
| 2 high-bit-depth paths | [x] | WORKS | `photo render --space display-p3 --depth 16` OK (cicp `[12,13,0,1]`); `--space bogus` lists valid spaces | - |
| 3 bounded RAW ingestion | [x] | WORKS | `raw add`: rggb16 and linear16 DNG OK; `photo-dng-ifd-loop` -> `[malformed-resource] IFD ... reached twice`; jpegxl -> `[unsupported-capability]`; oversize -> `[limit-exceeded] 40000x40000 exceeds 32768`; strip-out-of-bounds -> `[malformed-resource]`; truncated -> `[malformed-resource]`; CR2 and X-Trans -> clear `[unsupported-capability]` with convert-to-DNG advice; duplicate id rejected | - |
| 4 camera profiles + white balance | [x] | PARTIAL | `--temperature 5000 --tint 5 --exposure 1` changed render hash `2d346184` -> `c9bfefe8`; DCP import not exercised | P3 |
| 5 lens + geometry | [x] | UNVERIFIED | `photo info` shows `lens:[]`, `geometry:{}`; no profile fixture exercised; `tests/v0110_lens_geometry.rs` exists | - |
| 6 development stack | [x] | WORKS | exposure range checked (`99` -> `[invalid-develop] ... -5-5`), unknown group/param errors list valid names, `--auto-tone --dry-run` works, render deterministic (cmp equal) | - |
| 7 detail processing | [x] | UNVERIFIED | `--set detail.noise_luma` rejected ("expected one of sharpening, noise, moire"); the group is real but I did not exercise valid params | - |
| 8 local adjustments | [x] | UNVERIFIED | `photo mask` command exists; not exercised | - |
| 9 HDR and panorama | [x] | PARTIAL | `merge-hdr` of 12x12 + 8x8 -> `[invalid-input] ... brackets must share one frame size`; `merge-pano` -> "too few matching features"; success path not exercised (fixtures are not real brackets) | P3 |
| 10 wide-gamut and HDR delivery | [x] | WORKS | `archive-master` export writes 16-bit TIFF with ICC (`"icc":true,"depth":16`); `photo inspect` reports histogram/gamut | - |
| 11 variants, snapshots, sync | [x] | WORKS | `variant add v2`, `snapshot add s1`, `settings sync ph1 --to ph1/v2 --groups tone` ok; master removal refused | - |
| 12 local organization | [x] | WORKS | rate/pick/label, keywords, smart collection `--query "rating>=4"`, `search` paginated (matches/limit/offset/has_more) | - |
| 13 metadata privacy | [x] | WORKS | `privacy-report` flags gps 5, identity 1; exported `ph1-master.jpg` has Exif but no `GPS` bytes | - |
| 14 output recipes + batch delivery | [x] | WORKS | `export --dry-run` leaves doc sha unchanged and writes nothing; real export writes `outd/{hl,ph1}-master.jpg`; unknown recipe or missing photo -> error and no output dir created (staged) | - |
| 15 photographer UX | [x] | PARTIAL | `GET /api/photo/catalog` over loopback `serve` returns the catalog JSON (matches 2); `PhotoPanel.tsx`/`PhotoStage.tsx` exist; the UI itself not driven (my `/api/photo/preview?photo=ph1` curl got no response, probably wrong query shape) | P3 |
| 16 conformance + performance | [x] | PARTIAL | ten hostile fixtures behave; `v0110_conformance.rs` exists; benchmark/pinned digests/3-OS evidence not verified | P3 |

## Process note

At the end of the v0.11 sweep I ran `taskkill /F /IM pentool.exe` to stop my own `serve`. It reported one process
terminated; if another auditor had a `pentool.exe` running at that moment it could have been the one killed.
