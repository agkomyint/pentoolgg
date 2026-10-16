# v0.16.0 handoff notes from Claude (2026-10-16)

Read this before continuing. Written by a second agent that audited the tree
read-only while Codex was editing. I changed no source, test, or TASKS.md file,
to avoid colliding with in-flight edits. Please delete this file at release.

## Status (by checkbox)

Done: 1, 2, 3, 4, 5, 5a, 5b, 6, 7, 12 (10 of 18).
Open: 8, 8a, 9, 9a, 10, 11, 13, 14.

## Why progress feels slow

Tasks 8, 8a, 9, 9a and 10 each carry long "partial foundation" paragraphs in
TASKS.md, and each keeps growing. The remaining gaps named in them are
overwhelmingly items the roadmap itself puts out of scope under **Explicitly
deferred**: structural/manufacturing certification, score compression,
cutter conformance, DXF/Artios/Esko interchange, arbitrary-mesh unfolding.
Do not keep implementing those to satisfy a checkbox.

## Recommended finish plan (in order)

1. **Stop widening scope.** For each of 8, 8a, 9, 9a, 10, check the work
   against the **Acceptance targets** section (bottom of TASKS.md), not against
   production-grade packaging. The acceptance text explicitly allows
   "draft/reference-only" handoffs. If the targets pass, tick the box and
   shorten its prose to: what exists, what is deferred, where the tests are.
2. **Fix the one real defect on record.** Task 14 notes a bare generated
   carton preflight reports *five current-fold panel intersections* and
   `needs-review`. A stock built-in template that fails its own preflight is a
   genuine bug against "no unintended gaps or overlaps" (acceptance target on
   templates). Fix the template/solver, add a regression that preflights every
   built-in template at flat, midpoint and folded states and expects no
   intersections, or document precisely why a given case is expected.
3. **Task 11 (Studio).** The only item with no recorded browser-level
   acceptance. `web-ui/src/workspaces/PackagingWorkspaceStage.tsx` is
   untracked and new. Finish it, run `bun run build` to refresh
   `web/studio.html`, and do one desktop plus one 390x844 pass.
4. **Task 13 and 14 are evidence tasks, not coding tasks.** Their only
   unmet requirement is *hosted Linux + Windows + macOS CI* and a tagged
   draft-release run. Local Windows evidence is already extensive. Stop adding
   more local-evidence prose to these two items.
5. **Pre-push gate** once, from AGENTS.md: `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets --locked`.
6. **Commit locally in focused commits.** The tree currently holds many
   unrelated uncommitted changes (color/print v0.12 work, roadmap v0.22
   deletion, examples/, output/). Split v0.16 work from the rest. Do not
   commit `output/` or ad-hoc files in `examples/` without checking.
7. **Do not push.** The user's standing rule: push to GitHub only when they
   ask (CI costs money). Ask them once everything above is green locally;
   hosted CI is the last gate for 13 and 14.

## Things to avoid

- Do not tick 13/14 before hosted CI passes (AGENTS.md definition of done).
- Do not edit `docs/roadmap/v0.22.0/TASKS.md`: it shows as deleted in
  `git status`; confirm with the user whether that deletion is intended.
- `src/scene3d/pack.rs` is ~344 KB in one file. Consider splitting it only
  after release; do not refactor now.

## Environment note

A second `cargo test` using a separate `CARGO_TARGET_DIR` crashed rustc with
`STATUS_STACK_BUFFER_OVERRUN` (0xc0000409) while compiling the lib. Possibly
memory pressure from parallel builds. I did not investigate. If your own builds
start crashing like this, check free RAM and whether another build is running.
Tests are registered through `tests/suites/suite_12.rs`, so run them with
`cargo test --test suite_12 v0160`, not `--test v0160_packaging`.
