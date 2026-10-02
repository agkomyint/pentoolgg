# v0.5.0 — Local libraries and reusable components

Turn ordinary `.pen` content into reusable local assets. Humans must be able to
save, browse, preview, and place illustrations, icons, logos, signage, and UI
components. Agents must receive stable IDs, deterministic ordering, compact JSON,
and transactional operations.

v0.5 is local-first. It defines the asset and instance model that v0.6 will
package and distribute, but it does not add registries, downloads, accounts, or
automatic updates.

## Product invariants

- An asset is a valid `.pen` document with optional asset metadata, not a second
  drawing format.
- Ordinary `.pen` files without asset metadata remain valid and importable.
- Asset IDs are stable identity; filenames and absolute paths are not identity.
- Asset version and `.pen` format version are separate values.
- A library index and its previews are disposable caches, never source of truth.
- Adding an asset is transactional and uses the existing v0.4 composition engine.
- A placed instance remains renderable and exportable when its source library is
  missing, moved, disabled, or changed.
- Source changes never modify instances silently.
- CLI and browser use the same Rust discovery, extraction, preview, and placement
  services.

## Must ship, in order

- [x] **1. Specify asset metadata and identity.** Add an optional top-level
  `asset` object with a separately versioned schema. Define and validate stable
  `id`, name, description, asset version, kind, author, license, tags, category,
  entry page, insertion bounds/anchor, compatibility, and optionally exposed
  component properties with object/property targets.

  Use a documented portable ID grammar such as `namespace/name`. Reject duplicate
  IDs within a library. Unknown metadata fields must be preserved. Document the
  difference among file-format version, asset version, and content hash.

- [x] **2. Add asset authoring and extraction.** Save an entire document, one
  page, one or more layers, named objects, or a rectangular selection as a reusable
  `.pen` asset.

  ```sh
  pentool asset create design.pen --document --output assets/design.pen
  pentool asset create design.pen --page mobile --output assets/mobile.pen
  pentool asset create design.pen --page home --layer navbar \
    --output assets/navbar.pen --id ui/navbar --name "Navigation bar"
  pentool asset create design.pen --objects logo/mark,logo/wordmark \
    --output assets/logo.pen --id brand/logo
  pentool asset create design.pen --rect 100 200 400 300 \
    --output assets/card.pen --id ui/card
  ```

  Preserve stacking, stable object IDs, transforms, known and unknown fields, and
  only fonts required by the selection. Normalize the asset origin from calculated
  visual bounds without flattening editable content. Support `--dry-run`, explicit
  overwrite, atomic writes, and deterministic JSON summaries. The browser's “Save
  to library” action must call the same Rust engine.

- [x] **3. Add local library configuration.** Register, remove, refresh, inspect,
  enable, disable, and list folders without copying their contents into Pentool.

  ```sh
  pentool library add ./assets/icons --name icons --scope project
  pentool library add ~/design-assets --name personal --scope user
  pentool library list --format json
  pentool library refresh icons
  pentool library remove icons
  ```

  Store user configuration in the platform-standard config directory. Store
  project configuration in `.pentool/libraries.json` with project-relative paths.
  Missing, renamed, unreadable, or overlapping folders produce isolated warnings
  rather than corrupting configuration or the index.

- [x] **4. Build a fast, disposable asset index.** Incrementally index asset
  metadata, relative path, filename, tags, category, kind, dimensions, object
  types, modification data, compatibility, parse status, and content hash. Use an
  embedded persistent full-text index suitable for at least 10,000 assets.

  ```sh
  pentool explore arrow --library icons --tag navigation --limit 20
  pentool explore --category signage --kind illustration --format json
  pentool asset inspect icons/arrow-right
  pentool library doctor icons
  ```

  Support filters, pagination, cancellation, atomic refresh, and deterministic
  ordering with asset ID as the final tie-breaker. One malformed file cannot block
  the library. Bound file size, parse time, metadata length, and traversal; do not
  unintentionally follow paths outside a registered root.

- [x] **5. Generate deterministic previews.** Produce cached PNG and SVG previews
  through the native renderer. Key previews by renderer version, content hash,
  entry page, scale, and relevant render options. Write cache files atomically and
  invalidate them without relying only on timestamps.

  ```sh
  pentool asset preview icons/arrow-right --output arrow.png --scale 2
  pentool asset preview ./assets/logo.pen --output logo.svg
  ```

  Previewing must be browser-free. A broken preview records an isolated diagnostic
  and cannot block search or other previews.

- [x] **6. Place assets as editable copies.** Resolve a stable asset ID and build
  on v0.4 import to place it with page, position, scale, rotation, prefix, selected
  source layers, insertion position, canvas expansion, dry run, revision guard,
  and recovery snapshot.

  ```sh
  pentool add poster.pen icons/arrow-right --mode copy --at 400 200 --scale 0.5
  pentool add poster.pen ./assets/logo.pen --mode copy --page cover --dry-run
  ```

  Preserve editability and stacking. Deduplicate fonts, resolve ID conflicts
  deterministically, retain unknown fields, and return old-to-new ID maps. Paths
  are an explicit convenience; indexed stable IDs are the canonical interface.

- [x] **7. Add offline-safe component instances.** An instance records source
  library, stable asset ID, asset version, exact content hash, entry page,
  transform, visibility, and approved property overrides.

  ```sh
  pentool add poster.pen ui/button-primary --mode instance --at 400 200
  pentool instance inspect poster.pen instance-42
  pentool instance set poster.pen instance-42 --property color=#ff3366
  pentool instance detach poster.pen instance-42
  ```

  Each instance carries a materialized fallback snapshot sufficient for open,
  render, export, copy, and detach without a library. Only placement, visibility,
  and explicitly exposed properties are editable; other edits require detaching.
  Detach transactionally creates ordinary editable content without visual change.
  v0.5 may report a source mismatch but must not update instances; reviewed updates
  belong to v0.6.

- [x] **8. Build the browser Asset Explorer.** Add scoped navigation, search,
  filters, virtualized thumbnails, details and diagnostics, recents, local
  favorites, drag/drop placement, explicit copy/instance choice, and “Save
  selection to library.” Clearly identify unavailable sources, invalid assets, and
  instances currently using an embedded fallback.

- [x] **9. Test scale, safety, and portability.** Cover 10,000 assets, incremental
  refresh, duplicate IDs, renamed and overlapping folders, corrupt caches, invalid
  documents, preview failures, font deduplication, ID conflicts, project-relative
  paths, fallback rendering, overrides, detachment, cancellation, and cross-platform
  path/case behavior. Document human and agent workflows end to end.

## Out of scope for v0.5.0

- Archives, public catalogs, registries, network downloads, accounts, or payments.
- Publishing, installing, pinning, dependency resolution, signatures, or lockfiles.
- Silent or automatic instance updates.
- Scripts, executable hooks, remote fonts, or active content in assets.
- Full nested layout/component semantics beyond explicit properties and transforms.

## Acceptance demo

Save one layer and one multi-layer selection as assets. Register that project
library alongside a user library holding 10,000 icons. Refresh and search both
deterministically from CLI and browser, inspect metadata, and create a preview.
Place an icon as a copy and the multi-layer asset as an instance; transform it and
apply an approved color override. Disconnect the source and prove the document
still opens, renders, exports, copies, and detaches the instance. Reconnect the
library and report source state without changing the document automatically.
