# v0.6.0 — Shareable, reproducible, and trusted libraries

Extend the v0.5 local library and instance foundation into a safe ecosystem for
packaging, publishing, installing, reproducing, and explicitly updating `.pen`
assets across machines and teams. The protocol must be open: filesystem, Git,
self-hosted HTTP catalogs, and any future official service are peers.

v0.6 distributes the same assets defined by v0.5. It must not introduce a second
asset format or make remote availability necessary for rendering documents.

## Required v0.5 foundation

- Assets are valid `.pen` documents with stable IDs, asset versions, deterministic
  bounds, optional exposed properties, and content hashes.
- Instances contain source identity plus a materialized fallback snapshot.
- Local indexing, previews, copy placement, instances, and detach share Rust
  services across the CLI and browser.
- Source changes are detected but never applied silently.

## Product and protocol invariants

- Published package versions are immutable. Changed bytes require a new version.
- A lockfile identifies exact package versions, origins, and content hashes.
- Opening or exporting a document never performs network access or changes a lock.
- Installation and update are explicit, transactional, bounded, and reversible.
- Package contents are data only: no scripts, executable hooks, or active content.
- Checksums are mandatory; publisher signatures are optional but verifiable.
- Trust, provenance, license, and network requirements are visible before install.
- Component updates are reviewed operations with previews, conflict reports, and
  rollback. Missing sources never stop fallback rendering.
- CLI and browser share the resolver, verifier, cache, installer, and update planner.

## Must ship, in order

- [x] **1. Publish package, catalog, and registry specifications.** Define a
  versioned deterministic `.penpkg` archive containing a package manifest,
  content-addressed `.pen` assets, optional previews, checksums, license texts,
  and no executable content. Specify:

  - package coordinates (`namespace/name@version`) and immutable semantic versions;
  - package versus asset identity and versioning;
  - stable asset ID to content hash and relative path mappings;
  - dependencies and compatible Pentool/file-format ranges;
  - author, license, homepage, source, deprecation, and yanking metadata;
  - deterministic ordering, timestamps, normalization, and hashing;
  - detached publisher signatures and key identifiers;
  - a paginated, cacheable registry index and artifact-fetch protocol;
  - capability discovery and structured protocol errors.

  Keep the protocol host-neutral and publish JSON Schemas plus normative fixtures.
  Reject absolute paths, traversal, escaping links, duplicate entries, ambiguous
  Unicode names, and case-colliding paths.

- [x] **2. Add deterministic packaging and verification.** Reuse v0.5 asset
  validation and previews in browser-free commands.

  ```sh
  pentool package init ./my-icons --name community/my-icons
  pentool package pack ./my-icons --output my-icons-1.2.0.penpkg
  pentool package inspect my-icons-1.2.0.penpkg --format json
  pentool package verify my-icons-1.2.0.penpkg
  pentool package verify my-icons-1.2.0.penpkg --require-signature
  ```

  Identical normalized input must produce identical package bytes and hashes.
  Verification should stream, enforce compressed and expanded limits, validate
  every asset and preview, and report safe-to-collect diagnostics deterministically.

- [x] **3. Implement registries and package lifecycle commands.** Support multiple
  named filesystem, Git, and HTTPS sources with explicit priority and provenance.
  Add search, publish, install, update, pin, unpin, list, and remove.

  ```sh
  pentool registry add team https://design.example.com/index.json
  pentool package publish my-icons-1.2.0.penpkg --registry team --dry-run
  pentool library install community/my-icons@1.2.0 --registry team
  pentool library update community/my-icons --dry-run
  pentool library pin community/my-icons@1.2.0
  ```

  Publishing fails if a coordinate exists with different bytes. Installation
  verifies before atomic activation in the v0.5 index. Removal detects lockfile and
  instance references and requires explicit override; instance fallbacks remain
  renderable.

- [x] **4. Make installs reproducible.** Add a documented project lockfile with
  schema version, exact package coordinate, registry identity and canonical origin,
  package hash, asset hashes, dependency graph, and compatibility data.

  ```sh
  pentool library sync --locked
  pentool library sync --offline
  pentool lock verify
  pentool lock explain community/my-icons
  ```

  Resolution is deterministic and detects cycles and incompatible constraints.
  `--locked` fails rather than rewriting the lock. Offline sync works from a verified
  cache. Open, render, preview, and export never resolve packages, contact a
  registry, or mutate the lockfile. Configuration and lockfiles are cross-platform.

- [x] **5. Add a verified content-addressed cache.** Store packages and extracted
  assets by cryptographic hash, use atomic writes and inter-process locks, and
  separate download, quarantine, verified, and active states. Provide inspect,
  verify, repair, prune, and offline-status commands. Never trust cache filenames
  without revalidation. Pruning protects active locks and supports dry run.

- [x] **6. Add provenance, trust, and credential controls.** Expose signer,
  origin, license, compatibility, replacement, yanking, and verification state
  before installation. Support user/project policies for allowed registries,
  required signatures, and publisher keys.

  Supply authentication through the OS credential store, environment, or an
  external credential helper. Never write secrets to project configuration,
  lockfiles, logs, command output, crash reports, packages, or URLs. Configuration
  export must omit credentials.

- [x] **7. Implement explicit component update planning.** Compare an instance's
  recorded version/hash with installed candidates and produce a read-only plan:
  metadata and stable object/property changes, visual preview/diff, retained and
  invalid overrides, conflicts, compatibility, and provenance.

  ```sh
  pentool instance updates poster.pen
  pentool instance update poster.pen instance-42 --to 1.3.0 --dry-run
  pentool instance update poster.pen instance-42 --to 1.3.0
  pentool instance rollback poster.pen instance-42
  ```

  Applying an update is transactional, revision-guarded, and backed up. Replace the
  fallback only after verification, retain valid overrides, require explicit conflict
  resolution, and retain previous source metadata and fallback for rollback. Updating
  one instance never updates other instances or the lock implicitly. Batch update
  requires an explicit list or `--all` and returns per-instance results.

- [x] **8. Add remote discovery and lifecycle UI.** Extend the v0.5 explorer with
  clearly separated Local, Installed, and Remote results; registry, publisher,
  license, trust, compatibility, installed-version, and offline filters; package
  detail and history; install/update plans; visual diffs; conflict resolution;
  rollback; and cancellable progress. Never imply a remote result is installed or
  trusted.

- [x] **9. Support team and self-hosted operation.** Allow prioritized registries,
  mirrors, private catalogs, offline import/export of verified packages, health
  reporting, and reproducible bootstrap from project configuration plus lockfile.
  Define behavior for unavailable registries, stale indexes, mirror disagreement,
  yanked versions, rotated keys, and deleted upstream packages.

- [x] **10. Harden and audit the supply chain.** Test archive bombs, traversal,
  symlinks, malicious filenames/Unicode, MIME disagreement, oversized content,
  parser/renderer failure, slow sources, interrupted downloads, concurrent installs,
  dependency cycles, hash/signature failure, key rotation, registry compromise,
  cache corruption, and downgrade attempts. Bound memory, disk, redirects, retries,
  time, and concurrency. Keep a local append-only audit log for publish, install,
  update, rollback, trust-policy, and verification events without secrets.

- [x] **11. Document publishing and governance.** Define namespace ownership,
  publisher verification, version immutability, yanking versus deletion,
  deprecation, ownership transfer, moderation, vulnerability reporting, key
  rotation/revocation, compromised-release response, licensing, mirrors, and
  disaster recovery. Cover public, private, Git-only, filesystem-only, and offline
  teams.

## Out of scope for v0.6.0

- Silent background installation or component updates.
- Network access during document open, render, preview, or export.
- Package scripts, plugins, macros, build hooks, or embedded active content.
- Real-time multiplayer editing or cloud document storage.
- Ratings, comments, payments, and social features required for resolution.
- Requiring an official hosted registry for third-party interoperability.

## Stretch goals

- [ ] **12. Local collaboration metadata.** Collections, favorites, and private
  usage counts that never affect package hashes, resolution, or lockfiles.
- [ ] **13. Read-only web catalog.** Shareable package/asset pages that can open
  the Asset Explorer. Publishing remains an authenticated CLI/browser operation
  with the same verification flow.

## Acceptance demo

Create a multi-asset library with v0.5 extraction, package it twice, and prove
byte-identical output. Sign and publish 1.2.0 to a self-hosted registry, install and
lock it in a clean project, disconnect the network, and reproduce the same hashes
and export from cache. Publish 1.3.0, inspect provenance plus a visual/structural
update plan, preserve one valid override, surface one invalid override as a
conflict, explicitly update one instance, and roll it back. Reproduce the project
on another OS from configuration and lockfile, while proving documents with missing
packages still render from their v0.5 instance fallbacks.
