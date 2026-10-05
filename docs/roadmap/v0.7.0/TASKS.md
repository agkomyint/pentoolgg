# v0.7.0 — Raster images and non-destructive image editing

Add secure, deterministic image placement and adjustment without turning Pentool
into a pixel-painting application or compromising its single-binary, offline,
content-addressed design.

Normative proposal: [`IMAGE-SPEC.md`](IMAGE-SPEC.md).

## Implementation status

Status audited against `main` at `f3b38b4` on 2026-10-06. A checked box means the
entire item meets the repository definition of done; partially implemented items
remain unchecked.

| Item | Status | Implemented now | Still required |
| --- | --- | --- | --- |
| 1 | Complete | Image specification, threat model, v5 schema, and normative fixtures | — |
| 2 | Complete | v5 assets/nodes, explicit v4→v5 migration, validation, downgrade diagnostics, extension preservation | — |
| 3 | Complete | Bounded PNG/JPEG/WebP decoding, signatures, dimensions/pixels/memory limits, SHA-256, deduplication, EXIF orientation, alpha reporting, normalized metadata-free derived PNG | Additional codec golden coverage belongs to item 12 |
| 4 | Partial | Embedded assets, aggregate byte budget, safe relative external paths, symlink containment, mandatory hashes, offline missing/hash diagnostics | Integrate image blobs with the existing verified cache and locking paths |
| 5 | Partial | `image add/set/info/remove`, frame/crop/fit/position/opacity/transform/mask fields, transactions, history, dry-run, revision guards, JSON output, tree/search bounds | Prove and cover image behavior through generic batch, groups/components, align/distribute, and diff workflows |
| 6 | Partial | Authoritative PNG, portable SVG, raster-backed PDF, multi-page export, parent-space vector masks, atomic PNG/SVG output, source/hash/mask diagnostics | Explicit linked-SVG mode, fully atomic PDF output, and cross-platform fit/crop/mask pixel goldens |
| 7 | Not started | Empty operation stacks are accepted | Implement and specify every required pixel operation; nonempty stacks currently fail explicitly |
| 8 | Not started | — | Operation CLI and transactional batch support |
| 9 | Not started | — | Deterministic bake, provenance, predictions, and undo |
| 10 | Not started | — | Image-aware package/archive/cache integration |
| 11 | Not started | Import reports basic source metadata | Dominant colors, focal suggestion, versioned analysis, and palette handoff |
| 12 | Partial | Codec smoke tests, EXIF orientation, migration, masks, deduplication, relocation, and hash rejection run in Linux/Windows/macOS CI | Compact codec/operation pixel goldens, alpha/color edge cases, package/bake/undo/cache-corruption coverage, and explicit Intel/Apple Silicon evidence |
| 13 | Not started | — | Proxies, mip levels, and processed-result cache |
| 14 | Partial | v5 documents open and authoritative embedded/external images render in the browser canvas; image nodes appear in the object list | Crop/focal/mask handles, fit and adjustment controls, operation ordering, before/after, reset, bake confirmation, recovery UX, and preview-status UI |
| 15 | Not started | — | Image-aware cold/warm renderer benchmarks and memory/stage metrics |
| 16 | Partial | Bounded decoding, traversal/symlink/hash tests, malformed fixtures, checked allocation arithmetic | Fuzz targets, allocation/concurrency/cache/operation hostile suites, and dependency policy automation |
| 17 | Not started | Pure-Rust codecs preserve the single-binary architecture | Record and gate binary size on every release target |
| 18 | Partial | Format/spec/threat-model documents and introductory README image workflow | Complete operations, packages, cache, performance, bake, recovery, and error documentation with runnable batch examples |

Current usable slice: users can import, place, inspect, crop, transform, mask,
preview, remove, and export verified images without manually editing `.pen` JSON.
The complete v0.7.0 release is not ready until the unchecked work below and its
exit criteria are satisfied.

## Required foundation

- v0.6 package hashing, verified cache, immutable artifacts, and offline rendering.
- v0.6.1 instrumented renderer with object-, pixel-, and memory-sensitive metrics.
- v0.6.2 ordered scene graph, migration, unified transactions/history, batch create,
  groups/components, styles, structured errors, diff, and multi-page export.

v0.7 work starts only after those contracts are stable enough to avoid creating a
second object hierarchy, cache, history mechanism, or batch language.

## Milestone A — Alpha: image model and placement

- [x] **1. Freeze the image specification and threat model.** Review node schema,
  asset storage, processing order, color/alpha math, masks, resource limits,
  metadata policy, package representation, cache keys, errors, and migration.
  Publish JSON Schema and normative valid/invalid fixtures.

- [x] **2. Bump and migrate the `.pen` format.** Add image-asset records and image
  scene nodes through the v0.6.2 migration framework. Older documents remain valid;
  newer unsupported node kinds fail loudly. Add explicit upgrade, validation,
  downgrade/flatten diagnostics, and round-trip extension preservation.

- [x] **3. Implement bounded PNG, JPEG, and WebP import.** Use audited pure-Rust
  decoders. Verify signatures, dimensions, pixels, memory, orientation, color-space
  handling, alpha, metadata, and SHA-256 before commit. Deduplicate source blobs.
  Strip private metadata from derived output by default.

- [ ] **4. Implement embedded and external assets.** Enforce byte budgets for
  embedded data and secure document-relative paths plus mandatory hashes for
  external data. Reuse the v0.6 content-addressed cache and locking. Missing and
  mismatched assets produce stable structured errors; rendering never uses network.

- [ ] **5. Add image placement commands and shared APIs.** Ship `image add`, `set`,
  `info`, and remove with frame geometry, fit, focal position, crop, opacity,
  transform, and normal compositing. Integrate tree/search/object/groups/components,
  bounds, align/distribute, diff, history, `--dry-run`, revision guards, and JSON
  output.

- [ ] **6. Render images through every supported output.** Add authoritative Rust
  rendering for PNG, portable SVG, linked SVG where explicitly requested, and PDF.
  Implement vector masks with defined coordinate space and fill rules. Add missing
  source/hash/mask diagnostics and atomic export behavior.

### Alpha exit criteria

- Place the same embedded image twice without duplicating source bytes.
- Move the document plus external image directory and render the same verified
  pixels from the relative path.
- Reject a changed external file before decode and without partial output.
- Export fit/crop/mask fixtures identically on all four release targets.
- Older supported `.pen` fixtures still render unchanged.

## Milestone B — Beta: non-destructive operations

- [ ] **7. Implement the versioned operation engine.** Add crop, resize, rotate,
  brightness/contrast, levels, curves, hue/saturation, gaussian blur, sharpen, and
  grayscale with pinned parameter ranges, processing order, sampling, edge modes,
  rounding, clamping, sRGB behavior, and premultiplied-alpha rules.

- [ ] **8. Add operation CLI and batch support.** Ship `image op add|list|set|move|
  enable|disable|remove`. Extend batch so assets, image nodes, and complete operation
  stacks can be created transactionally. Failures report operation index and scene
  context and leave neither orphan blobs nor history entries.

- [ ] **9. Add deterministic bake.** `image bake` creates a new verified source,
  records provenance, applies the metadata policy, and atomically repoints selected
  nodes. Support dry-run size/hash predictions where possible and complete undo.

- [ ] **10. Integrate assets with packages.** Include image blobs and metadata in
  deterministic `.penpkg` archives and manifests. Verify hashes and decode limits
  during pack, publish, install, cache repair, and offline sync. Package operations
  never execute image tools or contact undeclared services.

- [ ] **11. Add deterministic analysis.** `image analyze` returns dimensions,
  aspect ratio, alpha, format, dominant colors, and versioned focal-point suggestion.
  Permit an explicit follow-up command to create/update v0.6.2 palette tokens;
  analysis itself remains read-only.

- [ ] **12. Add processing and decoder conformance tests.** Commit compact golden
  fixtures for every codec/operation, EXIF orientation, alpha edges, masks, color
  normalization, invalid sources, extreme parameters, migration, package round
  trips, bake, undo, and cache corruption. Test decoded pixel hashes on Windows,
  Linux, Intel macOS, and Apple Silicon macOS.

### Beta exit criteria

- A complete image node and operation stack can be created in one batch.
- Reordering or disabling one operation creates a clear structural diff and one
  history entry.
- Source bytes stay unchanged until explicit bake.
- Bake records source/result hashes and is fully undoable.
- Package twice from normalized input and obtain byte-identical `.penpkg` output.

## Milestone C — Release candidate: hardening and editor UX

- [ ] **13. Add content-addressed proxies and processed-result caching.** Generate
  bounded thumbnails/mip levels for editor responsiveness. Key every result by
  source, canonical operations, dimensions, masks, color/alpha contract, and engine
  versions. Cache loss or corruption may slow rendering but never change output.

- [ ] **14. Build browser crop, mask, and adjustment UI.** Provide focal-point and
  crop handles, fit controls, operation list/reordering, parameter controls,
  before/after preview, reset, bake confirmation, missing-source recovery, and clear
  approximate-versus-authoritative preview status. Use shared Rust services for
  committed results.

- [ ] **15. Add image-aware renderer benchmarks.** Extend the v0.6.1 harness with
  codec, source dimensions, output dimensions, image count, reuse count, operation
  stack, masks, cache state, scale, memory peak, decode, process, composite, encode,
  and I/O stages. Include cold and warm runs and editor proxy latency.

- [ ] **16. Fuzz and harden.** Add continuous fuzz targets for every decoder,
  schemas, operations, masks, cache records, migration, and package images. Test
  bombs, dimensions, integer overflow, allocation failure, traversal, hash mismatch,
  corrupt metadata, interrupted writes, concurrent cache access, and malicious
  operation stacks. Run dependency advisory/license/source checks in CI.

- [ ] **17. Enforce distribution budgets.** Record release-binary size per target,
  analyze growth, and gate unexpected regressions. Default artifacts stay single
  binaries with PNG/JPEG/WebP. Any `pentool-full` codecs require separate audited
  features and the same security/determinism matrix—no native runtime dependencies.

- [ ] **18. Document the complete workflow.** Publish import/storage tradeoffs,
  portability, crop/fit/mask semantics, every operation, metadata privacy, color
  limitations, packages, cache repair, performance, bake, migration, errors, and
  external-tool boundaries. Include runnable CLI and batch examples.

### Release-candidate exit criteria

- Editor interaction uses proxies and remains responsive with a documented stress
  fixture while authoritative export stays pixel-equivalent.
- Cold/warm benchmark results, peak memory, output sizes, and binary-size changes
  are published for every release target.
- Fuzzing and hostile-fixture suites complete under documented time and memory
  budgets with no crash, panic, escape, partial commit, or unbounded allocation.
- A clean offline machine installs a packaged image design from verified cache and
  reproduces the same decoded output hashes.

## v0.7 acceptance targets

- Import, place, crop, mask, adjust, group, componentize, diff, undo, package, and
  export raster images without manually editing `.pen` JSON.
- Reusing one source across 100 nodes stores one authoritative blob.
- Rendering performs zero network requests and rejects hash mismatches reliably.
- Default imports support PNG, JPEG, and WebP in one self-contained executable.
- All processing is non-destructive until `image bake`.
- All writes support dry run, revision guards, structured output, and one history
  entry through the v0.6.2 transaction engine.
- Pixel, memory, decode, and package limits are checked before unsafe allocation.
- EXIF/GPS does not survive derived/baked output unless explicitly requested.
- Normative image fixtures pass on Windows, Linux, Intel macOS, and Apple Silicon.
- Binary-size and render-performance changes are measured and published.

## Explicitly out of scope for v0.7

- Brushes, raster layers, erasers, selections, and a pixel-painting engine.
- Animation or implicit selection of animated-image frames.
- Remote URL fetching during open, render, preview, package install, or export.
- Bundled AI models, background removal, inpainting, or generative fill.
- Unspecified cross-platform blend modes or color-management claims.
- Required system libraries such as libvips, OpenCV, ImageMagick, or FFmpeg.

## Post-v0.7 protocol direction

Define a separately reviewed external-tool protocol for AI and specialist image
processors. It must be explicit, least-authority, content-addressed, provenance-
preserving, auditable, and incapable of executing merely because a document or
package was opened.

## Acceptance demo

Import one JPEG externally and one transparent PNG embedded; prove their hashes and
metadata policy. Place the JPEG into two grouped cards with different non-destructive
crops, focal points, masks, and adjustment stacks. Derive palette tokens through an
explicit analysis-to-style step, then batch-create additional instances. Show a
structural and visual diff, undo and redo an operation reorder, bake one copy while
the other retains its original source, package the design twice identically, install
it offline, and export every page to SVG, PNG, and PDF with matching golden pixels
on all release platforms. Finish by demonstrating bounded rejection of a malformed,
oversized, hash-mismatched source and showing that no partial document, cache, or
history state remains.
