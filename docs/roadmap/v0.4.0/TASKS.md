# v0.4.0 — Large documents and `.pen` composition

Make large layered projects practical, then allow one `.pen` document to be
composed safely into another. Keep the existing readable format and agent-first
CLI behavior; do not require the browser for import or export.

## Must ship, in order

- [x] **0. Add first-class pages.** Introduce format v3 with ordered pages, each
  owning a canvas and layers. Load v1/v2 files as one virtual page; add page-aware
  CLI editing/export, browser switching and management, and shared Rust API page
  selection without changing legacy artwork.

- [x] **1. Establish large-document budgets and benchmarks.** Add generated
  fixtures for 1,000 layers and 100,000 mixed path/text objects. Benchmark load,
  validation, search, one-object edits, import, SVG export, and PNG export while
  recording wall time and peak memory. Define release budgets for a reference
  machine and fail CI on severe regressions.

- [x] **2. Index objects instead of repeatedly scanning every layer.** Build a
  document index keyed by layer ID and object ID, use it for search and targeted
  edits, and update or rebuild it safely after mutations. Keep deterministic
  back-to-front ordering and reject duplicate or ambiguous IDs. Add compact CLI
  output and pagination (`--offset`, `--limit`) so agents do not receive enormous
  JSON responses.

- [x] **3. Add a transactional import command.** Support a browser-free command
  such as:

  ```sh
  pentool import destination.pen source.pen --prefix poster --at 400 120
  ```

  Import all source layers, paths, text, transforms, and required embedded fonts
  as one atomic operation. Preserve source stacking order and unknown extension
  fields. Validate both files first, support `--dry-run`, expected destination
  revision checks, and create the same numbered recovery snapshot used by batch
  editing.

- [x] **4. Specify collision, canvas, and font rules.** Default to a deterministic
  namespace prefix for imported layer/object/font IDs and report every mapping in
  machine-readable JSON. Reject collisions when `--prefix` is disabled; never
  silently overwrite. Apply `--at`, optional scale, and optional rotation without
  flattening editable geometry or text. Default to keeping the destination canvas;
  add an explicit `--expand-canvas` option. Deduplicate byte-identical fonts and
  rename different fonts that share an ID.

- [x] **5. Keep large files responsive in the browser.** Virtualize the layer and
  object tree, debounce indexed search, avoid rebuilding every row after a small
  edit, and render only visible canvas regions where possible. Show import
  preview, ID mappings, placement controls, progress, and actionable errors. The
  browser must call the same Rust import engine as the CLI.

- [x] **6. Harden memory, cancellation, and recovery.** Enforce configurable
  import/file/object limits before expensive work, avoid unnecessary full-document
  clones in hot paths, allow long render/import jobs to be cancelled, and ensure a
  failure never changes the destination. Test malformed inputs, locked layers,
  oversized files, interrupted writes, font conflicts, and imports near limits.

- [x] **7. Ship compatibility tests and documentation.** Prove v1/v2 files still
  open unchanged, imported results render identically to their source after the
  requested transform, and all release platforms produce deterministic ID maps.
  Document performance expectations, import examples, conflict policies, recovery,
  and the difference between linking and copying (v0.4 copies; live links are out
  of scope).

## Stretch goals

- [x] **8. Pages/artboards.** Introduce a format migration only after import and
  large-document behavior are stable. Old files must open as one page with no
  visual change, and import should be able to target a page.
- [ ] **9. Selective import.** Add `--layer`, repeated filters, or a manifest so an
  agent can import only named source layers without first rewriting the source.

## Acceptance demo

Import a 100,000-object source project into an existing destination with a prefix
and placement transform, dry-run the operation, commit it with a revision guard,
search the imported objects through paginated CLI output, edit one object, and
export successfully. The destination remains byte-for-byte recoverable from its
automatic backup, and the browser can inspect the result without freezing.
