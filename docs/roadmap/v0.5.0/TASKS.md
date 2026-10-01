# v0.5.0 — Local asset explorer and reusable components

Turn `.pen` files into reusable local design assets. Humans should be able to
browse and place icons, logos, illustrations, signage, and UI components, while
agents receive stable IDs and compact machine-readable results.

## Must ship, in order

- [ ] **1. Define an asset manifest.** Add optional metadata for asset ID, name,
  description, version, author, license, tags, category, preview, and recommended
  insertion bounds. Keep ordinary `.pen` files valid when metadata is absent.

- [ ] **2. Add local library management.** Register, remove, refresh, and list
  folders without copying their contents into Pentoolgg. Store configuration in a
  documented user-level location and support project-local libraries for portable
  teams. Missing folders must produce warnings rather than corrupt the index.

  ```sh
  pentool library add ./assets/icons --name icons
  pentool library list
  pentool library refresh icons
  pentool library remove icons
  ```

- [ ] **3. Build a fast asset index and search CLI.** Index metadata, filenames,
  tags, object types, dimensions, and modification time. Support filters,
  pagination, deterministic ordering, and compact JSON designed for agents.

  ```sh
  pentool explore arrow --library icons --tag navigation --limit 20
  pentool explore --category signage --format json
  pentool asset inspect icons/arrow-right
  ```

- [ ] **4. Generate deterministic previews.** Create cached PNG/SVG thumbnails
  using the native renderer, invalidate them when source content changes, and
  expose a browser-free preview command. A broken asset must not prevent the rest
  of a library from being indexed.

- [ ] **5. Place assets into an existing document.** Build on v0.4 import so users
  can add an indexed asset by stable asset ID with position, scale, rotation,
  prefix, target layer/page, dry-run, revision guard, and recovery snapshot.

  ```sh
  pentool add poster.pen icons/arrow-right --at 400 200 --scale 0.5
  ```

- [ ] **6. Introduce local components and instances.** Allow an imported asset to
  remain an editable copy or become an instance that records its source asset ID
  and version. Instances may override placement, visibility, and approved design
  properties. Detaching converts an instance to ordinary editable layers.

- [ ] **7. Create the browser Asset Explorer.** Add library navigation, search,
  filters, virtualized thumbnail grids, detail previews, drag/drop placement,
  recent assets, favorites, and clear copy/instance selection. Use the same Rust
  index and placement engine as the CLI.

- [ ] **8. Test scale and portability.** Cover large libraries, duplicate names,
  moved folders, stale previews, invalid assets, font deduplication, ID conflicts,
  instance detachment, and Windows/macOS/Linux path handling. Document how agents
  create, tag, discover, and place reusable assets.

## Out of scope for v0.5.0

- Public hosting, accounts, ratings, payments, and automatic network downloads.
- Silent instance updates. Local source changes require an explicit update action.

## Acceptance demo

Register a folder containing 10,000 `.pen` icons, search it from the CLI and
browser, preview an icon, place it as both a copy and an instance, move and scale
it, detach the instance, and export the result without manually opening or parsing
the source asset.

