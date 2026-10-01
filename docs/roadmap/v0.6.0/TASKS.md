# v0.6.0 — Shareable libraries and trusted asset packages

Extend the v0.5 local Asset Explorer into a safe ecosystem for publishing,
installing, versioning, and updating `.pen` libraries across machines and teams.
The registry protocol should be open so Git repositories and self-hosted catalogs
can work alongside any future official service.

## Must ship, in order

- [ ] **1. Specify the package and registry protocols.** Define a versioned
  package manifest, content-addressed asset files, preview files, checksums,
  dependencies, supported Pentoolgg versions, author information, license, and
  registry index format. Publish the specification before building a hosted
  service.

- [ ] **2. Add package lifecycle commands.** Support pack, verify, publish,
  install, update, pin, list, and remove with deterministic JSON output and useful
  dry runs. Permit filesystem, Git, and HTTPS registry sources.

  ```sh
  pentool package pack ./my-icons
  pentool package verify my-icons-1.2.0.penpkg
  pentool library install community/my-icons@1.2.0
  pentool library update community/my-icons --dry-run
  ```

- [ ] **3. Make installs reproducible.** Add a project lockfile containing exact
  package versions, registry origin, and content hashes. Support offline installs
  from a populated cache. Never change a pinned dependency during export or when
  opening a document.

- [ ] **4. Add provenance and trust controls.** Verify checksums and optional
  publisher signatures, display authorship and license before installation, and
  record the exact source of every installed package. Treat `.pen` packages as
  data: prohibit scripts and executable hooks. Warn clearly about unsigned or
  replaced releases.

- [ ] **5. Implement explicit component updates.** Detect newer source versions,
  show a structured change summary, preview visual differences, and update chosen
  instances transactionally. Preserve valid overrides and report conflicts rather
  than silently discarding user edits. Support rollback to the previous version.

- [ ] **6. Add remote discovery to the browser.** Provide searchable catalogs,
  publisher and license filters, preview galleries, install state, version history,
  update review, and offline status. Separate installed/local results from remote
  results so users always know whether an action requires a download.

- [ ] **7. Support team and self-hosted catalogs.** Allow multiple configured
  registries, authentication supplied through the operating system credential
  store or environment, private libraries, mirrors, and export/import of registry
  configuration without exporting secrets.

- [ ] **8. Harden the supply chain.** Enforce compressed and expanded size limits,
  safe archive paths, MIME/content validation, timeouts, cancellation, atomic cache
  writes, quarantine for failed verification, dependency-cycle detection, and
  protection against malicious metadata or preview files. Add an auditable install
  and update log.

- [ ] **9. Document publishing and governance.** Define namespace ownership,
  version immutability, deprecation, removal, moderation, vulnerability reporting,
  license expectations, and recovery when a registry is unavailable.

## Stretch goals

- [ ] **10. Collaboration metadata.** Favorites, collections, usage counts, and
  optional ratings without making them part of deterministic build output.
- [ ] **11. Web catalog.** A read-only public gallery whose URLs can be opened in
  Pentoolgg; publishing and editing still go through authenticated tooling.

## Acceptance demo

Publish a signed versioned icon library, install and lock it in a project, use an
asset offline, detect a newer version, inspect its metadata and visual diff, update
selected instances while retaining valid overrides, roll back, and reproduce the
same exported design on another machine from the lockfile.
