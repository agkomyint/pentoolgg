# Pentool Contributor and Agent Guide

This file is the operating contract for humans and coding agents working in this
repository. It applies to the whole tree unless a more specific `AGENTS.md` exists
below the file being changed.

## Project purpose

Pentool is a single-binary, offline-capable Rust vector editor built for both
people and software agents. It serves a browser editor, reads and writes `.pen`
documents, exposes deterministic CLI operations, and exports PNG, SVG, and PDF.

Protect these product properties in every change:

- one Rust binary with no required Node.js service, database, or network;
- readable, editable, versioned `.pen` JSON;
- deterministic behavior and stable object identifiers;
- atomic, recoverable document mutations;
- cross-platform behavior on Linux, Windows, Intel macOS, and Apple Silicon;
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

## Roadmap awareness

Roadmap documents describe intended work; they do not prove that a feature is
implemented. Confirm behavior in code and tests.

The current planned sequence is:

1. `docs/roadmap/v0.6.24/TASKS.md`: trustworthy component updates and overrides;
2. `docs/roadmap/v0.7.0/TASKS.md`: raster images and non-destructive image editing;
3. `docs/roadmap/v0.7.1/TASKS.md`: professional compositing;
4. `docs/roadmap/v0.7.2/TASKS.md`: BYOK image models;
5. `docs/roadmap/v0.7.3/TASKS.md`: pixel editing and retouching;
6. `docs/roadmap/v0.7.4/TASKS.md`: photography and RAW development;
7. `docs/roadmap/v0.7.5/TASKS.md`: color management and print production.

Earlier roadmap files are historical requirements and useful regression context.
When implementing roadmap work:

- follow its stated order when one task depends on another;
- preserve every invariant and acceptance target, not merely the example command;
- do not silently implement an item listed as out of scope;
- mark a checkbox complete only after implementation, tests, documentation, and
  required cross-platform evidence exist;
- update roadmap text when a deliberate design decision changes its contract;
- never bump a version just because a roadmap filename mentions that version.

If code, public documentation, schema, and roadmap disagree, stop and resolve the
contract explicitly. The schema and released behavior take precedence over an
unimplemented proposal.

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
- `tests/`: integration and rendering regression coverage.
- `docs/fixtures/`: normative, tracked fixtures. Keep them minimal and intentional.
- `examples/`: user-facing examples and their expected rendered outputs.
- `docs/roadmap/`: plans and acceptance criteria, not scratch notes.

## Development workflow

1. Inspect `git status` and preserve unrelated user changes.
2. Reproduce a bug before changing code. Add a regression test that demonstrates
   the actual failure mode.
3. Make the smallest coherent change in the shared implementation path.
4. Test malformed input, limits, pagination, filters, nested groups, pages, locks,
   and transaction rollback when relevant.
5. Update public docs, schema, fixtures, and CLI help when the contract changes.
6. Run the same checks used by CI before committing.

Required checks:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```

For release-sensitive changes also run:

```sh
cargo build --release --locked
cargo run --release --locked -- tree docs/fixtures/v4-scene.pen --limit 1
cargo run --release --locked -- search docs/fixtures/v4-scene.pen semantic --kind text
```

Run relevant export smoke tests when touching rendering, text, geometry, import,
document formats, or packaging. If local Rust and GitHub's current stable toolchain
differ, investigate hosted failures rather than weakening `-D warnings` globally.

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
Windows, and macOS. Release tags trigger four artifact builds and create a draft
GitHub release. Before tagging:

1. choose a new semantic version; never move or reuse a published tag;
2. update both `Cargo.toml` and the `pentool` package entry in `Cargo.lock`;
3. ensure the exact commit has passed regular CI;
4. verify release smoke coverage includes the changed behavior;
5. create the tag only from that passing commit;
6. monitor every target through packaging, checksums, and draft-release creation;
7. publish the draft only after artifact and release-note review.

Treat a green local run as necessary but not sufficient for a release. Hosted CI
uses the current stable Rust toolchain and is the cross-platform authority.

## Definition of done

A change is complete only when the implementation is coherent, regression coverage
passes, formatting and Clippy are clean, relevant docs/schema/help are current,
generated artifacts are absent, backward compatibility has been considered, and
the applicable roadmap acceptance criteria are demonstrably satisfied.
