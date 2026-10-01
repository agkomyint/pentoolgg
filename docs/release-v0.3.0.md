# v0.3.0 — Agent editing workflow

Pentoolgg v0.3.0 makes existing artwork much easier and safer for agents and
humans to inspect and edit.

- `search` and `tree --query` search layers, IDs, object types, and text, returning compact JSON,
  stable IDs, draw order, lock/visibility state, and native geometry bounds.
- `object` partially updates a path or text object without replacing unspecified
  properties. It also renames, duplicates, moves between layers, reorders, and
  removes objects using exact layer/ID targeting.
- `batch` applies JSON operations all-or-nothing, supports dry runs and expected
  revision checks, uses sibling-file replacement, and retains the exact previous
  document as a numbered `.bak.N` recovery snapshot.
- Unknown extension fields follow objects through rename, duplicate, move, and
  reorder operations.
- The browser layer panel now exposes searchable object rows with selection,
  rename, duplicate, forward/back reorder, and delete controls. These object
  edits use the same Rust operations as the CLI.
- Tests cover discovery, partial edits, locked layers, batch rollback, recovery,
  extension retention, typography, geometry, and stroke rendering.

The `.pen` format remains version 2 and v0.1/v0.2 documents remain readable.
Paths still render before text inside each layer; returned `draw_order` makes this
explicit. Pages/artboards remain planned for a later release so v0.3.0 can keep
editing semantics small, dependable, and backwards compatible.
