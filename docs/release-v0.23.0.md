# Pentool v0.23.0 — Fast Studio on heavy pages

No format, CLI or dependency changes. Rendered output is byte-for-byte
unchanged: 212 browser-core renders of 106 fixtures (two scales each, with the
new cache cold and warm) and the native PNG export comparison match v0.22.0
exactly.

## Speed

Measured on one machine with the same document: a 55-layer landscape of
blurred cloud and hill shapes, rendered by the browser core (WebAssembly).

| Render | v0.22.0 | v0.23.0 | Faster |
| --- | ---: | ---: | ---: |
| First render, 0.64 scale | ~1150 ms | ~390 ms | ~3x |
| Re-render after moving one layer, 0.64 scale | ~900 ms | ~100–125 ms | ~8x |
| First render, 1.0 scale | ~2800 ms | ~850 ms | ~3.3x |
| Re-render after moving one layer, 1.0 scale | ~2400 ms | ~250–300 ms | ~9x |

Dragging a selected object no longer waits for a render at all (see below).

## Changes

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
- Browser core: finished shape layers (fills, paths, rectangles, ellipses,
  lines, including their blur effects) are kept between renders, up to 96 MiB.
  An edit re-renders only the layers that changed. Resource limits are
  accounted exactly as if every layer were rendered again, so the same
  documents succeed or fail with the same errors.
- Browser core: built with WebAssembly 128-bit SIMD, which every current
  browser supports. Results are identical lane by lane.
- Blur effects render several times faster: the blur runs over rows in memory
  order with two buffers instead of copying the whole image for every channel
  and pass. A blur at full opacity now takes the blurred pixels directly.
- Gradient fills render several times faster: the gradient geometry is read
  once per fill instead of once per sample, and only the pixels the fill can
  cover are visited.
- Fully transparent and fully opaque pixels skip premultiplication arithmetic.
