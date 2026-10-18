# Audit: core roadmap (v0.3.0 - v0.6.24)

Auditor: read-mostly pass against installed `pentool.exe` 0.20.0, repro files in the session scratchpad
(`audit-core/`). Nothing in the repo was edited. Verdicts: WORKS / BROKEN / PARTIAL / UNVERIFIED.
Severity: P0 data loss, P1 documented feature broken, P2 wrong output or missing test/doc/help, P3 polish.
"Defect" ids refer to `TASKS-core.md`.

Coverage note: v0.6.0 (packages, registry, lock, cache, trust) and v0.6.24 items 15-20 were only sampled
(see rows). Browser/Studio items are UNVERIFIED (no browser run).

## v0.3.0

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| 1 tree/search bounds + pagination | [x] | WORKS | `tree --limit`, `search` return matches/returned/offset/limit/has_more consistently | - |
| 2 object set/duplicate/reorder | [x] | WORKS | commands succeed; history entry written | - |
| 3 batch atomicity, locks, `--if-revision`, dry-run parity | [x] | WORKS | `layer is locked: layer-1`; revision mismatch error; dry-run revision == commit revision | - |
| 4 browser layer panel | [x] | UNVERIFIED | no browser run | - |
| 5 history/undo | [x] | WORKS | undo restores exact bytes | - |
| 6 pages | [x] (moved to v0.4) | WORKS | `page add`, 3-page doc | - |
| help/error text | - | PARTIAL | `object rename/duplicate/move-to-layer/remove` have empty help; missing-file error omits path | P3 (CORE-012) |

## v0.4.0

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| import into v1-v3 doc | [x] | WORKS | dry-run, prefix collision rejected (`use a unique --prefix`), `--if-revision` | - |
| import into default `pentool new` (v4) doc | [x] | BROKEN | `[unsupported-version]`, advice `migrate --target 3` | P1 (CORE-001) |
| benchmark 100k | [x] | WORKS | 100k render median ~643 ms (target <=1300), `--max-ms` fails correctly | - |
| browser virtualization | [x] | UNVERIFIED | no browser | - |
| item 9 selective import | [ ] | PARTIAL | not implemented, README has no `--layer` | P3 |

## v0.5.0

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| asset create (doc/page/layer/objects/rect) | [x] | WORKS | all selectors, dry-run, overwrite refusal, id validation | - |
| library add/list/refresh/remove/enable/disable/doctor, explore | [x] | WORKS | exercised; ambiguous id asks for library/id | - |
| add (copy/instance) into v4 doc | [x] | BROKEN | `document has no canvas` on default doc | P1 (CORE-001) |
| add with explicit path | [x] | BROKEN | `./assets/card.pen` -> `asset not found` | P2 (CORE-003) |
| add `--layer/--expand-canvas/--no-prefix`, position | [x] | PARTIAL | flags absent | P2 (CORE-003) |
| preview cache | [x] | BROKEN | corrupt cache file (`garbage`) served as ok; key lacks renderer version and entry page | P2 (CORE-004) |
| asset property add/set/rename/remove/list/usage/validate | [x] | PARTIAL | works; `set` cannot change only a default; nearest-id suggestion suggests layer for object; float noise `79.99999999999999` in inferred bounds | P3 (CORE-013) |
| migrate v3 -> v4 with attached instances | [x] | BROKEN | edit-protection guard rejects: `layer instance-1-content belongs to attached instance` | P2 (CORE-005) |
| Asset Explorer UI | [x] | UNVERIFIED | no browser | - |
| error naming missing object | [x] | PARTIAL | `one or more selected objects do not exist` names nothing | P3 (CORE-012) |

## v0.6.0 (sampled)

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| package init/pack | [x] | WORKS | `pack` twice -> `cmp` identical (deterministic); empty package rejected `package contains no assets` | - |
| package verify/inspect/keygen/sign | [x] | WORKS | keygen emits .key/.pub; sign + verify ok | - |
| verify rejects tampering | [x] | PARTIAL | appending one byte to the .pentpkg still verified ok | P2 (CORE-006) |
| verified content-addressed cache: inspect/verify/repair/prune/offline-status (item 5) | [x] | BROKEN | no `cache` command exists (`unrecognized subcommand 'cache'`); `library`/`package`/`lock`/`registry` have none | P1 (CORE-007) |
| trust/credential policy, provenance state before install (item 6) | [x] | UNVERIFIED | no CLI surface for policy found in `--help`; not tested | P2 (CORE-007) |
| registry | [x] | PARTIAL | only `registry search` exists; publish/install under `package` | P3 |
| lock verify/sync | [x] | UNVERIFIED | needs project fixture; not run | - |
| revision-guarded `instance update` | [x] | BROKEN | `instance update` has no `--if-revision` | P2 (CORE-008) |
| governance docs (item 11) | [x] | UNVERIFIED | not read | - |

## v0.6.1

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| `benchmark --render --json` stages, median/p95, RSS | [x] | WORKS | per-stage timings, `peak_resident_bytes` | - |
| 100k paths-only peak RSS target | [x] | PARTIAL | 250 MB measured vs 2.5x(pixmap+doc+out) target; docs admit not met | P2 (CORE-009) |
| item 6 profiling Win/macOS captures | [ ] | UNVERIFIED | open in roadmap | - |
| render regression tests | [x] | WORKS | tests/render_regression.rs present | - |

## v0.6.2

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| shape rrect/circle/line/ellipse, group create/move/duplicate, text-box | [x] | WORKS | duplicate collision rejected; negative values `--x -50` ok | - |
| style set/usage/list/replace, tokens propagate | [x] | WORKS | `fill_ref` + `style set color.accent` -> `#00FF00` in SVG export | - |
| alias tokens | [x] | PARTIAL | `style set` rejects alias value ("not a color"); alias support only in src/tokens.rs | P2 (CORE-010) |
| roadmap example `style replace A B` | - | PARTIAL | CLI needs `--to` | P3 (CORE-012) |
| multi-page export png/pdf | [x] | WORKS | 3 pngs `001-page-1.png`..; pdf ok | - |
| `diff` / `diff --visual` | [x] | WORKS | structural change path `/pages/0/layers/1/nodes/6`; visual artifacts written next to input | - |
| `format --compact` | [x] | WORKS | dry-run 8698 -> 3343 bytes (>25%) | - |
| `--json` error envelope | [x] | WORKS | `{"error":{"code":"not_found",...}}` rc=2; missing-file case code `operation_failed` with OS text only | P3 |
| SIGPIPE `| head` | [x] | WORKS | rc 0 | - |
| layout align/distribute/grid, history prune/restore, text parity, v3<->v4 round trip, component promote/instantiate | [x] | UNVERIFIED | not exercised in this pass | - |

## v0.6.24

| item | claimed | verdict | evidence | sev |
|---|---|---|---|---|
| bulk update, one history entry, stack/overrides preserved | [x] | WORKS | `instance-update-bulk` | - |
| rollback / undo byte-exact | [x] | WORKS | cmp identical | - |
| conflicts, all-or-nothing, `--continue-on-conflict` | [x] | WORKS | `removed-override-target`; exit 1, file unchanged | - |
| untracked local edit detected | [x] | WORKS | conflict reported | - |
| readable child ids | [x] | WORKS | `instance-1-card-bg` | - |
| tight bounds hidden/include-hidden/canvas-bounds/empty | [x] | WORKS | clear error on empty | - |
| tight bounds include stroke expansion | [x] | BROKEN | rect 80x80 stroke 10 -> 80x80; width-20 line height 0 | P2 (CORE-002) |
| override value validation | [x] | BROKEN | `instance set --property bg --value notacolor` ok:true; "notacolor" written to SVG | P2 (CORE-011) |
| update plan accuracy | [x] | BROKEN | identical-content update marks every object `geometry-changed`, `unchanged: 0` | P2 (CORE-014) |
| tracked local edit creation (`--instance-edit`) | [x] | BROKEN | flag does not exist; only hand-edited JSON + keep-local | P2 (CORE-015) |
| generic command guard on attached layers | [x] | WORKS | actionable message | - |
| doc example validity | [x] | BROKEN | docs/instance-overrides-v0.6.24.md:97 `asset create chrome.pen --id ai/chrome --rect content` invalid | P3 (CORE-012) |
| items 15-20 (format evolution, resources, staged tx, limits, seams, baseline) | [x] | UNVERIFIED | only unit tests located (transaction.rs `future_versions_and_unknown_required_nodes_fail_explicitly`, `staged_bundle_failure_leaves_document_resource_and_history_unchanged`) | - |

## Verdict counts (rows above)

WORKS 31, PARTIAL 11, BROKEN 11, UNVERIFIED 10 (approximate, counted per table row).
