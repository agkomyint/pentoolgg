# Pentool v0.22.1 — Responsive Studio on heavy pages

A patch release. No format, CLI or dependency changes. Rendered output is
byte-for-byte unchanged.

## Fixes

- Studio: the local (WebAssembly) preview now renders in a background worker.
  Previously each render ran on the page's main thread, so on a heavy page (for
  example many blurred layers) every edit or drag froze clicks, scrolling and
  typing for seconds until the preview finished. A newer canvas render now also
  replaces an older one that has not started yet, so fast edits do not queue up
  stale previews, and the page on screen renders ahead of thumbnails.
- Studio: dragging an object no longer waits for a render. Selecting an object
  prepares the page below it, the object, and the page above it at the
  displayed size; a drag only slides the object between the other two, so it
  follows the pointer at once whatever the number of layers. Parts that did
  not change (the page around an object that was just moved) are reused.
- Blur effects render several times faster: the blur runs over rows in memory
  order with two buffers instead of copying the whole image for every channel
  and pass.
- Gradient fills render several times faster: the gradient geometry is read
  once per fill instead of once per sample, and only the pixels the fill can
  cover are visited.
