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
