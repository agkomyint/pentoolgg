# Pentool v0.6.23

Pentool v0.6.23 is a v4 correctness release.

- `new` creates v4 documents directly.
- `canvas` and `path` edit v4 documents without routing through the legacy model.
- batch paths and text accept either flat style fields or nested `style` values;
  flat values are normalized to the v4 style representation.
- `stroke_ref` is retained, rendered, reported by style usage, and exposed by
  tree/search summaries.
- bounded text contributes its declared box width and height to group bounds.
- instance updates and rollback preserve materialized layer positions.
- ordinary CLI errors render concise context without Rust stack backtraces.

See `docs/authoring-v0.6.2.md` and `docs/pen-format-v4.md` for the v4 authoring
model. Shapes, paths, and text all use the same nested `style` representation on
disk; batch operations may use convenient flat fields which Pentool normalizes.
