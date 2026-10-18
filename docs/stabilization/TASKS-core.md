# Stabilization tasks: core (v0.3.0 - v0.6.24)

Source: `audit-core.md`. Repro dir: session scratchpad `audit-core/`. "Patch-safe" = fix only, no format/CLI
contract change. Run repros with a v3 doc via `pentool new X.pen && pentool migrate X.pen --target 3` where noted.

## CORE-001 add/import fail on default (v4) documents
- **Status: fixed in 0.20.1.** Reproduced. Chose the actionable-error branch (no format change in a patch): `add` into a v4+ destination fails with `[unsupported-version]` naming `group promote/instantiate --component` and `migrate --target 3` (`import` already named its alternatives); README states the v3-instance vs v4-component split. Test: `tests/v070_hardening.rs::add_into_a_scene_graph_document_names_the_component_workflow`. Native v4 `add` remains a maintainer decision.
- Severity: P1. Needs maintainer decision (support v4 natively vs. auto-route to components).
- Repro: `pentool new d.pen; pentool import d.pen other.pen` -> `[unsupported-version]`; `pentool add d.pen <registered-asset>` -> `document has no canvas`.
- Expected: works, or fails with a message that names the supported path (`migrate --target 3` or `group promote`/`instantiate`). Actual: `add` message is misleading.
- Suspects: src/document.rs ~40-55 (Deserialize), src/main.rs `Command::Add` ~6872, src/import.rs compose, src/instance.rs.
- Test: tests/importing.rs `add_into_v4_document_is_supported_or_explains` ; tests/v064_instances.rs `add_on_v4_reports_actionable_error`.
- Accept: no "document has no canvas" for a valid v4 file; doc states the instance model split (v3 instance vs v4 component).

## CORE-002 asset tight bounds exclude stroke
- P2, patch-safe (doc already claims strokes included).
- Repro: v3 doc, rect 80x80 `--stroke-width 10`, `asset create` -> bounds 80x80; line width 20 -> height 0.
- Suspect: src/asset.rs `tight_visible_bounds`.
- Test: src/asset.rs unit `tight_bounds_include_stroke_expansion` (rect + line).
- Accept: rect -> 90x90 at -5,-5; line height = stroke width.

## CORE-003 `add` lacks path fallback and placement flags
- P2. `add ./assets/card.pen` -> `asset not found` (only `library::resolve`, while asset inspect/preview use `resolve_asset_path` with path fallback). Missing `--layer`, `--expand-canvas`, `--no-prefix`, position (roadmap v0.5 item 6).
- Suspect: src/main.rs `Command::Add`, `resolve_asset_path` (~11299).
- Test: tests/libraries_packages.rs `add_accepts_explicit_path`.
- Accept: path works; flags exist or roadmap text updated (decision needed for new flags).

## CORE-004 preview cache not validated, key incomplete
- P2, patch-safe.
- Repro: write `garbage` into `.pentool/previews/<hash>-<scale>x.png`; `asset preview` returns ok.
- Suspect: src/main.rs `AssetAction::Preview` (~6615) uses `cache.exists()` only; non-atomic write.
- Test: tests/libraries_packages.rs `corrupt_preview_cache_is_regenerated`.
- Accept: key includes renderer version + entry page; invalid cache regenerated; atomic write.

## CORE-005 migrate --target 4 rejects docs with attached instances
- P2. Repro: v3 doc with `instance`, `migrate --target 4` -> `layer instance-1-content belongs to attached instance`.
- Suspect: edit-protection guard applied to migrate; src/instance.rs.
- Test: tests/v064_instances.rs `migrate_v3_to_v4_with_instances`.
- Accept: migration succeeds preserving instance metadata, or error names a corrective command. Decision needed on v4 representation.

## CORE-006 package verify accepts trailing bytes
- P2 patch-safe. Append `x` to a verified .pentpkg; `package verify` still ok.
- Suspect: src/package.rs archive reader.
- Test: tests/libraries_packages.rs `verify_rejects_trailing_data`.
- Accept: verify fails; hash compare covers whole file.

## CORE-007 no package cache / trust / policy CLI
- P1 (roadmap v0.6.0 items 5, 6 are [x]). `pentool cache` absent; no inspect/verify/repair/prune/offline-status; no trust policy command. Needs maintainer decision: implement or uncheck items and edit docs.
- Test: tests/libraries_packages.rs `cache_verify_repair_prune` once decided.
- Accept: commands exist with dry-run prune honoring locks, or roadmap unchecked.

## CORE-008 `instance update` has no `--if-revision`
- P2. v0.6.0 task 7 requires revision-guarded updates. Other mutating commands accept the global flag; check it is honored/rejected loudly here.
- Test: tests/v064_instances.rs `instance_update_if_revision_mismatch`.
- Accept: mismatch fails, file unchanged.

## CORE-009 100k paths-only peak RSS target unmet
- P2, needs decision (relax target or optimize). 250 MB measured; docs/performance.md admits it.

## CORE-010 alias tokens cannot be created via CLI
- P2. `style set alias --type color --value color.accent` -> "not a color". Suspect src/tokens.rs vs CLI parse. Test: tests/v062_authoring.rs `style_set_alias_token`. Needs decision on syntax.

## CORE-011 override value not validated
- P2, patch-safe. `instance set --property bg --value notacolor` ok:true and exported.
- Suspect: src/instance.rs set/resolution (~735-800), property type from asset.rs.
- Test: tests/instance_flags.rs `instance_set_rejects_invalid_color`.
- Accept: error, file unchanged.

## CORE-012 docs/help drift (P3, patch-safe)
- docs/instance-overrides-v0.6.24.md:97 invalid `asset create` example; README claims import keeps `.bak.N` (actual: hidden history, `backup: null`); roadmap `style replace A B` needs `--to`; empty help for `object rename/duplicate/move-to-layer/remove`; missing-file error lacks path; missing selected object not named; `explore`/`library` roadmap flags differ.
- Test: a doc-example smoke test running README commands.

## CORE-013 asset property polish (P3)
- `property set` needs `--target`; nearest-id suggestion crosses kinds; inferred bounds show float noise (round to 1e-6).

## CORE-014 update plan reports false geometry changes
- P2 patch-safe. Update to identical-content 0.1.1: all objects `geometry-changed`, `unchanged: 0`, `changed: true`.
- Suspect: src/instance.rs plan compares transformed base/local vs untransformed incoming.
- Test: src/instance.rs unit `identical_source_update_plans_no_changes`.
- Accept: unchanged == object count, changed false.

## CORE-015 no tracked local-edit creation path
- P2, decision needed. `--instance-edit` absent (only in roadmap); users must hand-edit JSON. Either add a command or amend roadmap/acceptance demo.
