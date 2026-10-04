# v0.6.24 — Trustworthy component updates and overrides

Make asset instances safe to use across real multi-page designs. Updating an
instance must preserve placement and stacking, retain compatible overrides, detect
local edits honestly, and support document-wide updates in one transaction.

This is a correctness release. No update may silently change unrelated layout,
discard accepted user work, or claim `conflicts: []` when materialized instance
content differs from its recorded source.

## Required foundation

- v0.5 asset identity, materialized fallback instances, detach, and explicit update.
- v0.6 package provenance, immutable versions, hashes, locks, and rollback metadata.
- v0.6.2 ordered scene graph and unified history/transaction engine where available.

If v0.6.24 ships before every v0.6.2 authoring feature, it must still use stable
container/index metadata and one shared transactional write path. It must not create
a temporary second instance model.

## Update invariants

- An update preserves the instance's page, parent container, layer/group position,
  z-index, instance transform, visibility, lock state, and stable instance ID unless
  the user explicitly requests a placement change.
- Source content, materialized local content, and candidate source are compared as a
  three-way merge using stable object and property identities.
- Declared compatible overrides survive source updates.
- Undeclared local edits are never silently discarded. They become conflicts or are
  rejected when the edit is attempted.
- Dry run and committed update use the same planner and return the same change and
  conflict model.
- Updating many instances is one bounded transaction with one history entry and no
  partial document state.

## Must ship, in order

- [ ] **1. Reproduce and lock the reported regressions with tests.** Add fixtures
  for a card below a check-circle layer, edited instance text, a declared label
  override, a source-only geometry change, six pages using the same asset, content
  smaller than its source canvas, and readable child IDs. Tests must fail against
  v0.6.0 for the reported reasons before implementation changes begin.

- [ ] **2. Preserve stacking and container position during update.** Record and
  reuse the exact page, parent, and zero-based child/layer index. Replace instance
  content in place rather than remove-and-append. Add coverage for front, middle,
  and back positions; nested groups; locked/hidden siblings; repeated updates; and
  rollback. Render before/after overlap fixtures proving unrelated foreground
  objects remain visible.

- [ ] **3. Define and document the override schema.** Publish supported property
  types, targets, defaults, labels, constraints, stable keys, serialization,
  compatibility rules, and package behavior. Initial required properties:

  - text content;
  - fill and stroke color;
  - visibility;
  - numeric values such as stroke width, opacity, and primitive dimensions;
  - token/style references when supported by the document format.

  An exposed property addresses a stable source object ID plus a property path. An
  asset revision must not silently retarget an existing property key to a different
  semantic object or incompatible type.

- [ ] **4. Add complete CLI authoring for exposed properties.** Nobody should need
  to hand-edit asset JSON. Support repeated flags for simple cases and a JSON file
  for complete schemas:

  ```sh
  pentool asset create chrome.pen --id ai/chrome --rect content \
    --property 'counter=text:counter-label.content' \
    --property 'active=visibility:active-segment.visible'
  pentool asset property add chrome.pen ai/chrome counter \
    --target counter-label --field content --default '1 / 6'
  pentool asset property list chrome.pen ai/chrome --json
  pentool asset property validate chrome.pen ai/chrome
  ```

  Add set, rename, remove, inspect, and usage commands with dry run, revision guards,
  structured errors, nearest-ID suggestions, and package validation. `--help` and
  the format documentation must include runnable examples.

- [ ] **5. Prevent invisible local mutations.** Instance internals are protected by
  default. Generic `object`, `text`, `path`, shape, group, and batch commands must:

  1. reject edits inside an attached instance with guidance to use a declared
     override, detach, or an explicit advanced edit mode; or
  2. record the mutation as a tracked local patch with base revision and property
     identity when `--instance-edit` is explicitly supplied.

  Commands must never accept an ordinary edit that the updater cannot later detect.
  Existing documents containing untracked edits still require update-time detection
  and cannot be grandfathered into silent data loss.

- [ ] **6. Implement a real three-way update planner.** Compare:

  - base: the exact source hash/snapshot from which the instance was materialized;
  - local: current materialized content plus declared overrides/tracked patches;
  - incoming: the selected verified source revision.

  Classify added, removed, moved, renamed, type-changed, geometry-changed,
  style-changed, text-changed, locally changed, overridden, and unchanged objects.
  Report conflicts with instance ID, page, parent, source object ID, property path,
  base/local/incoming summaries, and available resolutions. `conflicts: []` is valid
  only after every local difference has been classified.

- [ ] **7. Preserve compatible overrides across updates.** An explicit override wins
  over a changed source default when its target and type remain compatible. Source
  changes to unrelated geometry or styling proceed normally. Report a conflict only
  when the target is removed, identity is ambiguous, type/constraint becomes
  incompatible, or both a tracked local patch and source change cannot be merged.

  Update instance fallback content first in memory, reapply overrides by stable
  property key, validate the final materialization, and commit atomically. Never
  require users to clear all overrides merely because the source changed.

- [ ] **8. Add explicit conflict resolution and policy.** Support `keep-local`,
  `take-source`, `detach`, `map-target`, and compatible per-property resolutions.
  Resolutions are included in dry-run plans and may be supplied through a JSON file
  for reproducible batch updates. Unresolved conflicts block commit; they never
  choose a destructive default.

- [ ] **9. Add document-wide and scoped bulk updates.** Implement:

  ```sh
  pentool instance update deck.pen --all --asset ai/chrome --dry-run
  pentool instance update deck.pen --all --asset ai/chrome --to 1.3.0
  pentool instance update deck.pen --page page-3 --asset ai/chrome
  ```

  Support asset, current version/hash, page, group, instance ID, package, and stale-
  only filters. Produce per-instance plans plus aggregate counts. Default behavior is
  all-or-nothing; an explicit `--continue-on-conflict` may update only conflict-free
  instances but must list skipped instances and still commit through one transaction.

- [ ] **10. Compute useful default asset bounds.** Without `--rect`, derive tight
  visible content bounds from selected asset nodes, including stroke expansion,
  transforms, text, primitives, images, and nested groups. Ignore hidden nodes by
  default with an explicit inclusion flag. Report the inferred bounds during create
  and inspect. Use the full source canvas only through `--canvas-bounds`; never as an
  undocumented fallback. Empty or unmeasurable content is a clear error requesting
  an explicit rectangle.

- [ ] **11. Generate readable, stable instance child IDs.** Prefer deterministic
  IDs derived from instance ID and source child ID, such as `card-2/label`. Escape
  unsafe characters canonically and add a short hash suffix only for a real
  collision. IDs must remain stable across placement, update, package reinstall,
  save/load, and operating systems. Expose source ID and materialized ID together in
  tree/search output so agents never need to reverse a hash.

- [ ] **12. Stop backup-file proliferation.** Route placement, overrides, update,
  detach, rollback, and bulk update through the shared hidden project history from
  v0.6.2. One user operation creates one history entry regardless of instance count.
  Until hidden history is available, add an explicit bounded backup policy and
  `--no-backup` only when another recoverable transaction mechanism is active. Never
  leave one adjacent `.bak.N` file per placed instance by default.

- [ ] **13. Improve plans, diagnostics, and documentation.** Human and JSON plans
  must show preserved placement, source transition, child changes, override outcome,
  local edits, conflicts, resolutions, fallback replacement, history/backup result,
  and affected pages. Document the asset/instance file format, property authoring,
  edit protection, merge rules, bulk updates, bounds, IDs, rollback, and recovery.

- [ ] **14. Add cross-platform conformance and visual regression coverage.** Run
  update, override, conflict, bounds, ID, bulk, history, package, and rollback tests
  on Windows, Linux, Intel macOS, and Apple Silicon. Golden renders must prove stack
  preservation and compatible override survival.

## v0.7 readiness gate

These tasks prepare the shared architecture for the
[`v0.7.0`](../v0.7.0/TASKS.md) image milestone without adding image nodes, codecs,
or editing features to this patch release. They are release requirements for
v0.6.24 because v0.7 must extend one scene graph, transaction system, package
protocol, cache, and renderer rather than introduce parallel infrastructure.

- [ ] **15. Freeze format-evolution and capability rules.** Document how new v4+
  node kinds, resource records, operation schemas, and renderer capabilities are
  versioned and validated. Unknown required kinds must fail loudly with a stable
  structured error; unknown optional extension data must survive round trips.
  Add forward-version, unknown-kind, extension-preservation, migration, and
  downgrade-diagnostic fixtures.

- [ ] **16. Stabilize one resource and content-addressing interface.** Extract and
  test the shared contracts for embedded bytes, document-relative external files,
  SHA-256 identity, deduplication, verified cache lookup, missing resources, and
  hash mismatches. v0.6.24 does not add image resources, but packages and future
  resource types must be able to reuse these contracts without a second cache or
  resolver. All paths must remain traversal-safe and offline by default.

- [ ] **17. Make transaction boundaries resource-safe.** Define a staged-write API
  that can atomically commit a document plus related cache/resource changes, with
  dry run, revision guards, one hidden-history entry, rollback, and cleanup after
  failure. Add fault-injection tests proving validation, interrupted writes, and
  conflicts leave no partial document, orphan resource, or misleading history
  entry.

- [ ] **18. Centralize bounded-work and structured-error policy.** Publish and test
  shared limits for input bytes, decoded/expanded dimensions, object counts, batch
  operations, recursion, memory estimates, and output size. Define stable error
  codes for unsupported capability, limit exceeded, malformed resource, hash
  mismatch, missing resource, and unsafe path. Limits must be checked before large
  allocation or mutation and be suitable for reuse by v0.7 decoders.

- [ ] **19. Freeze renderer and scene-extension seams.** Ensure bounds, tree,
  search, groups, components, layout, diff, history, batch, export, and package
  validation dispatch through documented node/resource interfaces. Add a harmless
  test-only extension fixture proving an added node kind can participate or fail
  explicitly without bypassing validation. Do not add raster rendering in this
  release.

- [ ] **20. Publish the v0.7 baseline and handoff report.** Record per-platform
  release binary size, representative render time, peak-memory method, package
  determinism hash, supported format versions, and the complete v0.6.24 conformance
  result. Link every v0.7 required foundation to its implementation and tests, and
  list any accepted debt explicitly. v0.7 development starts only after this gate
  and tasks 1–14 pass.

## Acceptance targets

- Updating a card below a check circle leaves the card at the same stack index and
  the check circle visible in the exported image.
- Editing attached instance internals through a generic command is rejected with an
  actionable override/detach suggestion; an explicitly tracked local edit appears
  in the next dry-run plan.
- The counter label and active segment can be declared as exposed properties using
  documented CLI commands only—no manual JSON editing.
- A per-instance label override survives an unrelated source geometry/style update.
- A source removal or incompatible change to an overridden target produces a real,
  inspectable conflict and blocks commit until resolved.
- Six instances across six pages update with one command, one transaction, and one
  history entry while retaining their individual overrides and stacking positions.
- An 80 x 80 visible asset reports approximately its content bounds rather than a
  920 x 400 source canvas unless `--canvas-bounds` is explicit.
- Materialized child IDs are readable and stable across two updates and a package
  reinstall.
- Dry-run and commit plans agree, rollback restores the exact pre-update bytes, and
  failed/conflicted updates leave no partial state or orphan backup.
- Unknown required scene capabilities fail explicitly, while optional extension
  data survives a validated save/load round trip.
- A simulated document-plus-resource failure leaves original bytes, cache contents,
  and hidden history unchanged.
- v0.6.24 publishes cross-platform binary-size, render, package-determinism, and
  compatibility baselines that v0.7 can measure against.

## Out of scope

- Silently guessing resolutions for genuine merge conflicts.
- Automatically exposing every source property as overridable.
- Updating every project on disk or contacting registries during document open.
- Replacing asset/package identity or the content-addressed cache.
- Real-time collaborative merge of simultaneous editors.
- Raster image nodes, codecs, image assets, masks, adjustments, or image editor UI;
  those remain in v0.7.0.

## Acceptance demo

Create a reusable chrome asset with CLI-declared `counter`, `active`, and color
properties. Place it on six pages below independent foreground check marks, apply a
different counter override on each instance, make one explicit tracked local edit,
and publish a new source revision with unrelated geometry plus one incompatible
property change. Run one document-wide dry run: it must preserve all stack indices,
retain compatible overrides, identify the tracked edit, and report exactly the
incompatible conflict. Resolve it explicitly, commit all six updates as one history
entry, verify the rendered checks remain visible, then undo and reproduce the exact
pre-update document.
