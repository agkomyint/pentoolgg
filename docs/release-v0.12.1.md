# Pentool v0.12.1

A Studio-focused patch release on top of v0.12.0. The `.pen` format is unchanged
(still up to version 8); no existing document or CLI flag changes behavior.

## Studio

- The Classic editor is retired. Photo and raster paint panels now live in
  Pentool Studio, the only browser UI. `/classic` returns 404.
- New documents start at the newest format (v8), so images, compositing, photo
  and print work without a manual upgrade.
- The header shows the document format and an **Upgrade** button
  (`POST /api/migrate`, the same migrations as `pentool migrate`). Upgrading is
  undoable and nothing is written until Save.
- Paint has an **Add paint layer** button (`POST /api/raster` action `add`), so a
  new document can be painted without the CLI.
- Moving an object follows the pointer immediately: the object and the page
  behind it are server-rendered once on grab and the object moves as a layer.

## Server

- Identical preview renders are answered from a bounded, digest-keyed cache.
  Exports and benchmarks are unaffected.
- The edit-to-preview debounce dropped from 120 ms to 30 ms.

## Build

- `cargo build --profile fast` is a local-testing profile with parallel codegen
  and incremental builds. Release artifacts keep the `release` profile.
