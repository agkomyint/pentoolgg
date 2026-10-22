# Pentool 0.24.2 (internal, unreleased)

Patch notes. Not published: the next public release is 0.25.0.

## Fixes

- Parent-page items now paint in PNG, SVG, PDF and Studio renders. They were
  resolved by `parent resolve` but never drawn. They render beneath the page's
  own layers, honour overrides and detaches, and hidden parent layers.
- `pentool spread <doc> facing <spread> true|false` accepts its value. The
  argument was parsed as a flag, so facing could not be turned on.

## Known gaps (not patches)

- Placed tables are not laid out or drawn by the renderer; only their story
  text is. Table layout needs a decision before work starts.
- Paragraph styles have no colour property.

## Also in 0.24.2

- `layout-grid guide-remove --help` documents that a guide ID and `--page` are
  exclusive (#169).
- Unreadable inputs to `import` and `extension add/run` report
  `[missing-resource]` like the other commands (#207).
- `pentool serve` checks the document exists before binding and never prints "listening" on failure; a busy port reports `[conflict]` with a `--port` hint (#160).
- `extension run` rejects an unknown `--page` (`[missing-resource]`) and non-object `--args` (`[invalid-input]`) before running the module (#205, #206).
- `batch` `put-shape` no longer silently drops a fill or stroke that is not a color string, and `put-path` rejects a paint object that has neither `fallback` nor `ref` (it used to save a shape that rendered with no fill). Both fail with a message and leave the document unchanged. Gradients are written as `{"fallback":{...}}` in `style`.
- A node with `visible: false` (and everything inside a hidden group) is now left out of PNG, SVG and PDF output for v4 documents instead of being rendered anyway (#396). Layer-level hiding already worked.
- `layout-grid step-repeat --help` states the count range (1 to 1000) and `raster select-load --help` lists the accepted modes (#388, #380).
- `ai connect --credential-env` now rejects a value that is not an environment variable name (such as a pasted key) with `invalid-credential-env`, saves nothing and never echoes it (#357).
- `layer-geometry` positional numbers (translate, rotate, scale, transform, hit, move-anchor, set-handle) accept negative values without `--` (#394).
- `shape --help` lists the geometry each kind requires (#399); `benchmark --help` states the layer and object ranges (#350).

- Spot-only `color separate --composite` jobs retain their configured or explicit CMYK destination, so the preview renders the spots over process paper (#397).
