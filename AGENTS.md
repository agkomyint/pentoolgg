# Pentool Contributor and Agent Guide

This file is the operating contract for humans and coding agents working in this
repository. It applies to the whole tree unless a more specific `AGENTS.md` exists
below the file being changed.

## Project purpose

Pentool is a single-binary, offline-capable Rust vector editor built for both
people and software agents. It serves a browser editor, reads and writes `.pen`
documents, exposes deterministic CLI operations, and exports PNG, SVG, PDF,
and flattened PSD.

Protect these product properties in every change:

- one Rust binary with no required Node.js service, database, or network;
- readable, editable, versioned `.pen` JSON;
- deterministic behavior and stable object identifiers;
- atomic, recoverable document mutations;
- cross-platform behavior on Linux, Windows, and Apple Silicon macOS (Intel
  macOS is no longer built or tested as of the release after v0.12.0);
- bounded work and compact machine-readable output for large documents;
- backward compatibility unless an explicit migration says otherwise.

## Start here

Before editing, read the smallest relevant set of sources:

- `README.md` for supported user workflows and public CLI examples;
- `docs/pen-format-v4.md` and `docs/pen-format-v4.schema.json` for the current
  ordered scene graph contract;
- `docs/pen-format.md` for legacy document behavior;
- `docs/PROTOCOL.md` for browser/server coordination;
- the applicable file in `docs/roadmap/` for scope, invariants, acceptance tests,
  and explicit exclusions;
- `.github/workflows/ci.yml` and `.github/workflows/release.yml` before changing
  build, packaging, fixtures, or release behavior.

Search the implementation and tests before designing a new abstraction. Extend
the shared scene, transaction, history, rendering, and validation paths instead of
creating a parallel model for one command.

## Current phase: stabilization (0.2x)

All roadmap milestones through v0.25.0 have shipped (the v0.20.0 items below are
the collaboration debt that remains). The product isfeature-complete for now. The job is to make what exists correct, tested,
documented and maintainable, and to ship the result as patch releases
(patch releases of the current minor).

In scope:

- bug fixes with a regression test each;
- finishing roadmap items that are shipped but not fully ticked (see
  "Open roadmap debt" below) and ticking a box only when implementation, tests,
  docs and cross-platform evidence all exist;
- behavior-preserving refactors (splitting oversized files, removing duplication);
- test reliability, docs/help accuracy, and performance regressions.

Out of scope until the maintainer says otherwise:

- new features, commands, flags, `.pen` fields, or dependencies;
- anything listed under "Explicitly deferred" in a roadmap file;
- time-based media (keyframe animation, video, audio), which has no roadmap.

A fix that needs a format or CLI change is not a patch. Stop and ask.

### Open roadmap debt (v0.20.0)

Verify against code and tests before working on these; they may have moved.

- Item 3a: `pentool collab compact` and the test that stats survive compaction.
- Item 7: one-click conflict resolution and presence cursors in Studio, and a
  live-browser check of the Collab workspace.
- Known collaboration gaps in `docs/release-v0.20.0.md`: no Ed25519 operation
  signing, no stale-base rejection for agents.

Closing the first two finishes a promised item and counts as stabilization.
Signing and stale-base rejection are new capability: ask first.

### Roadmap documents

Files in `docs/roadmap/` describe planned work; they do not prove a feature is
implemented. Confirm behavior in code and tests. Earlier roadmaps are historical
requirements and regression context: preserve every invariant and acceptance
target, and use them to audit shipped behavior. Mark a checkbox complete only
after implementation, tests, documentation, and required cross-platform evidence
exist.

GPU work uses `wgpu` as an optional `gpu` Cargo feature. The deterministic CPU
renderer stays normative; goldens and digests come only from the CPU path.

`pentool serve` is the authoritative backend and Studio (`web-ui/`) is the only
browser UI. `docs/roadmap/architecture/TASKS.md` records the shared-core/WASM
plan; do not start a crate split without its decision record.

If code, public documentation, schema, and roadmap disagree, precedence is:
released behavior and tests, then the schema, then public docs, then the roadmap.
Fix the lower source to match the higher one and record the decision in the
commit message. Stop and ask only when a fix would change released behavior.

## Versioning policy

Follow semantic versioning (`MAJOR.MINOR.PATCH`, no leading zeros):

- PATCH (`0.20.0` -> `0.20.1`): bug fixes, refactors without behavior change, and
  documentation only. No new features, no format or CLI changes beyond fixing
  documented behavior. This is the only kind of release in the stabilization phase.
- MINOR (`0.20.x` -> `0.21.0`): new backward-compatible features. While the
  version is below 1.0, a minor release may also carry a documented, migrated break.
- MAJOR (`0.x` -> `1.0.0`): declared stable `.pen` format and CLI.
- Fixes for a released version are made from its tag and shipped as the next patch.
- Batch several verified fixes into one patch release; do not cut a release per
  commit, because each release costs a full three-OS CI run.

## Repository map

- `src/main.rs`: CLI definitions and command routing. Keep business logic out when
  it can live in a focused library module.
- `src/scene.rs`: v4 scene graph, semantic nodes, migration, validation, grouping,
  bounds, and v4 inspection.
- `src/document.rs`: legacy v1-v3 typed document model.
- `src/transaction.rs` and `src/history.rs`: atomic writes and recovery.
- `src/render.rs`, `src/pdf.rs`, and `src/fonts.rs`: export pipeline.
- `src/agent.rs`, `src/editing.rs`, `src/geometry.rs`, and `src/text.rs`: discovery
  and editing behavior.
- `src/asset.rs`, `src/instance.rs`, `src/library.rs`, and `src/package.rs`: reusable
  assets and packages.
- `web/`: browser UI shipped inside the binary.
- `web-ui/`: TypeScript source (Vite, React, Tailwind, shadcn-style) of Pentool Studio;
  `bun run build` refreshes the committed, embedded `web/studio.html`. See
  `docs/studio.md`.
- `tests/`: integration and rendering regression coverage.
- `docs/fixtures/`: normative, tracked fixtures. Keep them minimal and intentional.
- `examples/`: user-facing examples and their expected rendered outputs.
- `docs/roadmap/`: plans and acceptance criteria, not scratch notes.

## Development workflow

1. Inspect `git status` and preserve unrelated user changes.
2. Reproduce a bug before changing code. Add a regression test that demonstrates
   the actual failure mode, and watch it fail before the fix.
3. Make the smallest coherent change in the shared implementation path.
4. Test malformed input, limits, pagination, filters, nested groups, pages, locks,
   and transaction rollback when relevant.
5. Update public docs, schema, fixtures, and CLI help when the contract changes.
6. Run the same checks used by CI before committing.

### Bug-fix recipe

1. Find the owner in the feature map below; read that module and its tests.
2. Reproduce with the smallest CLI command or test, using a temp directory.
3. Write the failing test, fix it, run only that test file.
4. If the fix changes a public JSON shape, error code, or `.pen` field, stop: that
   is not a patch.
5. Add a line to the notes for the next patch (`docs/release-v0.X.Y.md`).
6. Commit with an imperative message (`fix: ...`, `refactor: ...`, `test: ...`).

### Feature map

| Area | Code | Tests |
| --- | --- | --- |
| Scene graph, tree/search | `src/scene.rs`, `src/agent.rs` | `tests/suites/`, `docs/fixtures/` |
| Transactions, history | `src/transaction.rs`, `src/history.rs`, `src/stale.rs` | `tests/suites/` |
| Rendering, text, PDF | `src/render.rs`, `src/text.rs`, `src/pdf.rs`, `src/fonts.rs` | goldens in `tests/`, `examples/` |
| Collaboration | `src/collab/*` (op, log, merge, sync, perm, token, stats, presence), `src/comments.rs` | `tests/collab_*.rs`, `tests/comments.rs` |
| Studio UI | `web-ui/src/`, built to `web/studio.html` | `bun run typecheck`, embed checks in `tests/` |
| Server | `src/server.rs` | route scan in `tests/collab_sync.rs` |

Rules that bite:

- Every new or changed HTTP route must be classified in `perm::ROUTES`; the
  `every_route_is_classified` test fails otherwise.
- `web/studio.html` is generated. Never edit it by hand: change `web-ui/` and run
  `bun run build`, then commit both.
- Converged collaboration peers may differ in `collab.head` and `collab.seq`
  (chain positions are peer-local). Compare documents with those removed, never
  by raw bytes.
- Compare collaboration revisions with `collab::content_revision`, not
  `transaction::revision`, because the embedded `collab` block is derived data.

### Verification

Verification is tiered; do not run the full suite during iteration.

Inner loop (every edit; seconds to ~1 minute). Each `tests/*.rs` file is a
separate binary, so target the one file or test you touched:

```sh
cargo test --test <touched_test_file> --locked
cargo test --lib <name_filter> --locked
cargo check --all-targets --locked
```

Pre-push gate (once, before committing or pushing):

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

The full suite takes over ten minutes locally and the 3-OS CI matrix about
twenty. They are the authority for merge/release, not for iteration. Local
builds use `rust-lld` on Windows MSVC (`.cargo/config.toml`) and line-only
debuginfo (`[profile.dev]` in `Cargo.toml`); release artifacts keep the `release`
profile unchanged.

Timing-sensitive tests: the `collab_sync` and `collab_evidence` suites start real
servers and can fail with `[peer-unreachable] ... deadline has elapsed` when other
builds are running. Re-run alone or with `-- --test-threads=2` before treating it
as a regression. If a test is flaky on its own, fix its timing assumptions; do not
weaken the assertion.

For release-sensitive changes also run:

```sh
cargo build --release --locked
cargo run --release --locked -- tree docs/fixtures/v4-scene.pen --limit 1
cargo run --release --locked -- search docs/fixtures/v4-scene.pen semantic --kind text
```

Run relevant export smoke tests when touching rendering, text, geometry, import,
document formats, or packaging. If local Rust and GitHub's current stable toolchain
differ, investigate hosted failures rather than weakening `-D warnings` globally.

### Parallel agents

Give each agent its own git worktree and a disjoint set of files. Files that
conflict on nearly every merge are the `[[test]]` entries in `Cargo.toml`,
`README.md` sections, the command enums in `src/main.rs`, `perm::ROUTES`, and
`web/studio.html`. Agents must not edit `web/studio.html`; the integrator rebuilds
it once after merging. Merge by keeping both sides, then re-run the touched test
files before committing the merge.

### Refactoring

Refactors must not change behavior or output bytes. Prefer splitting oversized
files (`src/main.rs`, `src/collab/sync.rs`) along existing seams, and move code in
a separate commit from any edit to it. Run the full gate for any refactor that
touches more than one module.

## `.pen` compatibility rules

- Detect a document version before deserializing into a version-specific model.
- Versions 1-3 remain supported through the legacy model and explicit migration.
- Version 4 is an ordered scene graph with nested nodes. Do not flatten it merely
  to make an operation convenient unless the command explicitly targets v3 export.
- Unknown future versions must fail clearly and without mutation.
- Preserve unknown extension fields when the relevant format contract promises it.
- Migration must be explicit, deterministic, validated, and covered in both
  directions that the CLI claims to support.
- IDs must remain unique, nonempty, stable, and useful to agents.
- Page selection must affect only the selected page unless `--all-pages` or an
  equivalent explicit option is used.

Tree and search output are public agent APIs. Keep them compact, deterministic,
correctly paginated, and consistent across document versions. `matches`, `returned`,
`offset`, `limit`, and `has_more` must describe the emitted result exactly. Include
stable IDs, kinds, hierarchy information, bounds, and layer state without dumping
entire scene nodes.

## Mutation and safety rules

- Parse and validate into memory before writing.
- Use the shared transaction/history mechanism for user document mutations.
- A failed command must leave the original document byte-for-byte unchanged.
- Dry run and commit must use the same planner and report compatible results.
- Respect locks, revision guards, size/count limits, and path containment.
- Never discard local edits, overrides, placement, stacking, or extension data
  silently.
- Do not add adjacent backup-file proliferation; use bounded hidden history.
- Do not contact the network during document open, validation, or rendering.
- Reject unsafe archive paths and document-relative paths that escape their root.

## Tests and fixtures

Put behavior tests near the owning module when private helpers matter; use `tests/`
for public API, CLI-level, cross-module, and regression behavior. Every bug fix
should include a focused regression test whenever practical.

Fixtures must be deterministic and small. Do not leave ad-hoc `.pen`, PNG, SVG,
PDF, archive, executable, debug-symbol, benchmark, or smoke-test output in the
repository root. Use a temporary directory for generated test data. User-facing
artifacts belong in `examples/`; normative machine fixtures belong in
`docs/fixtures/`; transient artifacts belong in `target/`, `dist/`, or the system
temporary directory and must remain untracked.

Rendering changes require semantic assertions plus golden or pixel-level coverage
where visual output can change. Never refresh a golden blindly: explain why every
changed pixel is expected.

## CLI and diagnostics

- Preserve existing flags and JSON fields unless a documented compatibility break
  is approved.
- Prefer structured JSON results suitable for agents.
- Errors should name the operation, page, layer/group, object, and corrective
  action when available.
- Validate limits before expensive work.
- Keep `--help` examples runnable and synchronized with the README.
- Avoid successful no-ops that conceal a bad ID, unsupported version, or ignored
  filter.

## Dependencies and performance

Prefer the standard library and existing dependencies. A new dependency needs a
clear product benefit, compatible license, maintained upstream, bounded behavior,
and consideration of binary size and supply-chain risk. Keep `Cargo.lock` committed
and synchronized with `Cargo.toml`.

Avoid unnecessary document clones, repeated parsing, quadratic tree walks, and
unbounded output. Use the benchmark tooling and `docs/performance.md` for changes
that may affect large scenes or rasterization. Compare like-for-like release builds
on the same machine and record enough context to reproduce claims.

## Commits, CI, and releases

Keep commits focused and use imperative messages such as `fix: paginate v4 search`.
Do not commit generated binaries or local history. Do not rewrite or delete user
work to obtain a clean tree.

CI must pass formatting, Clippy with warnings denied, and all locked tests on Linux,
Windows, and macOS. Release tags trigger three artifact builds (Linux x64, Windows x64, Apple
Silicon macOS) and create a draft
GitHub release. The release workflow does not rerun the test suite: it first
requires a successful CI run for the tagged commit, then builds the release
binaries and smoke-tests them. Before tagging:

1. choose a new semantic version; never move or reuse a published tag;
2. update both `Cargo.toml` and the `pentool` package entry in `Cargo.lock`;
3. ensure the exact commit has passed regular CI;
4. verify release smoke coverage includes the changed behavior;
5. create the tag only from that passing commit;
6. monitor every target through packaging, checksums, and draft-release creation;
7. publish the draft only after artifact and release-note review.

Fuzzing is not a push gate: `.github/workflows/fuzz.yml` runs weekly and on
demand. CI's `binary-size` jobs build the exact release binaries and upload them;
the release workflow reuses those for the tagged commit (falling back to a build),
so the push-to-published flow is CI plus packaging, not two full LTO builds.

Treat a green local run as necessary but not sufficient for a release. Hosted CI
uses the current stable Rust toolchain and is the cross-platform authority.

## Definition of done

A change is complete only when the implementation is coherent, regression coverage
passes, formatting and Clippy are clean, relevant docs/schema/help are current,
generated artifacts are absent, backward compatibility has been considered, and
the applicable roadmap acceptance criteria are demonstrably satisfied.
