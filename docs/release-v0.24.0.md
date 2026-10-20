# Pentool v0.24.0 — Agent previews and a faster CLI (in development)

Plan and acceptance criteria: `docs/roadmap/v0.24.0/TASKS.md`. Exported bytes
are unchanged; new flags and JSON fields are additive.

## Changes

- `--preview` on every mutating command and its dry run: one call edits the
  document and returns a PNG of the changed page, rendered by the normal
  export renderer at 1024 px on the long edge. The JSON result gains a
  top-level `preview` object with `path`, `page`, `width`, `height`, `scale`,
  `revision` (always the revision the same call wrote) and `dry_run`.
  `--preview-size PX` (1 to 8192) sets the long edge, `--preview-out PATH`
  chooses the file, `--page` chooses the page (otherwise the first page that
  changed), and `PENTOOL_PREVIEW=1` turns previews on for a whole session.
  Images go to `.pentool/preview/` beside the document, which keeps the 32
  newest. A failed preview is reported as `preview.error` and never fails an
  edit that already committed. Without previews every command prints exactly
  the same bytes as before; the Studio server never renders CLI previews.
- Previews zoom in on the change: when the edited or added objects cover
  less than half the page, the image shows them plus padding at up to 8x and
  the result adds `focus` (`ids`, `x`, `y`, `width`, `height`) and a 256 px
  whole-page `thumbnail`. The focused image is a crop of the normal render at
  the reported scale, so every pixel matches `export --scale`. `--focus ID`
  (repeatable) picks the objects and `--no-focus` shows the whole page; an
  unknown ID is reported as `preview.error`.
- `pentool preview DOC` renders the document as it is on disk, read-only,
  with the same `preview` object (no `dry_run`): the whole page, or the
  `--focus` objects with a thumbnail; `--page`, `--max-size PX` and
  `--out PATH` as above.
- `--annotate` draws object IDs and bounds on the preview image (never on
  exports or the thumbnail) and adds `annotations` with `labels` (`id`,
  `color`, box in preview pixels) and `truncated`. It labels every object on
  a whole-page preview, or the focused objects, at most 50, and implies
  `--preview`.
- `--compare REV|FILE|before` renders the same view of a history revision, a
  `.pen` file or the state before the change, and adds `compare` with
  `changed_pixels`, `changed_pct`, `changed_bbox` and, when pixels changed, a
  `diff` image (changed pixels in red). It implies `--preview`.
- `export --max-size PX` picks the export scale so the longest page edge is at
  most `PX` pixels (1 to 8192; with `--all-pages` the largest page decides),
  instead of the agent guessing `--scale`. The result adds `max_size` and the
  chosen `scale`. It cannot be combined with `--scale`, and a bound that would
  need a scale below 0.1 fails with `[limit-exceeded]` before anything is
  written. Exports without it print the same bytes as before.
- Previews and exports of composite (v6) documents keep finished layer
  surfaces in `.pentool/cache/render-*` beside the document, so the next CLI
  call re-renders only the layers that changed (warm preview of a 55-layer
  1200x675 landscape: 335 ms to 209 ms). Output bytes are identical with and
  without it. Entries are checksummed (a damaged one is a miss), the
  directory is bounded to 256 MB by least recent use and is replaced when the
  binary changes. `--no-cache` or `PENTOOL_CACHE=0` turns it off.
