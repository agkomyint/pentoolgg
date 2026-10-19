# Pentool v0.22.1 — Responsive Studio on heavy pages

A patch release. No format, CLI or dependency changes.

## Fixes

- Studio: the local (WebAssembly) preview now renders in a background worker.
  Previously each render ran on the page's main thread, so on a heavy page (for
  example many blurred layers) every edit or drag froze clicks, scrolling and
  typing for seconds until the preview finished. A newer canvas render now also
  replaces an older one that has not started yet, so fast edits do not queue up
  stale previews.
