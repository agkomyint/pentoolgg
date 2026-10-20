# Pentool v0.24.1

Patch release. No format, CLI, or output changes.

## Fixes

- Studio page thumbnails of a multi-page v3–v5 document all showed the first
  page. The server's PNG cache keyed on the document JSON, which holds every
  page whichever is selected, so the first page rendered was served for every
  page. The cache key now includes the selected page. Only `pentool serve`
  used that cache; CLI `export` and `preview` were not affected.
- Studio created new documents at format v11 and offered "Upgrade format" only
  up to v11, although v12 has shipped. New Studio documents now start at v12.
- Studio rendered v10–v12 documents on the server instead of in the browser.
  The in-browser renderer handles them byte-identically, so 3D, bindings and
  gradient documents get the fast local path that v6–v9 already had. A test
  now fails if either Studio constant falls behind the newest format.

## Known limitation

`pentool new` still creates a v4 document. Formats v6 and later export SVG as
an embedded raster and PDF as raster pages (a nine-page deck: 48 KB vector PDF
at v4, 25 MB at v12), so starting the CLI at the newest format would change
released output. Vector SVG/PDF export for compositing documents is the
prerequisite for making the format invisible.
