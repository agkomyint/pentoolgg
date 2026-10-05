# Pentool v0.6.24

Pentool v0.6.24 makes reusable asset instances safe to update across real,
multi-page documents and freezes the shared foundations needed by v0.7.

## Trustworthy instance updates

- update plans compare the accepted source snapshot, current materialization, and
  incoming revision, and classify source/local changes instead of reporting false
  clean plans;
- compatible text, color, visibility, numeric, and token-reference overrides
  survive source changes;
- ordinary commands reject edits inside attached instances with actionable
  override/detach guidance;
- `keep-local`, `take-source`, `detach`, and `map-target` resolutions are explicit
  and reproducible through JSON;
- scoped `--all` updates plan from one original snapshot, update in memory, and
  commit once, preventing partial documents and cross-instance false conflicts;
- page, parent/index, z-order, visibility, lock state, stable instance ID, and
  readable child mappings survive updates and rollback;
- one hidden-history entry covers a bulk update, and undo restores exact bytes.

## Asset authoring and inspection

- exposed properties can be added, set, renamed, removed, listed, inspected,
  validated, and traced from the CLI with dry runs and revision guards;
- assets infer tight visible bounds by default; hidden content and full-canvas
  bounds require explicit flags;
- tree/search expose source and materialized IDs together.

## v0.7 foundation

- future format versions and required unknown node kinds fail explicitly;
- a traversal-safe, offline, SHA-256 resource contract is reusable by packages
  and future resource-backed nodes;
- staged resource/document commits validate before mutation and recover resource
  bytes when the guarded document commit fails;
- bounded-work and stable structured-error policy is documented in
  [`v0.7-foundation-v0.6.24.md`](v0.7-foundation-v0.6.24.md).

The release workflow tests and packages Linux x64, Windows x64, Intel macOS, and
Apple Silicon, publishes platform baseline metadata, and creates a draft GitHub
release with archives and SHA-256 checksums.
