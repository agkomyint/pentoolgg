# v0.6.1 — Faster, observable, and predictable rendering

Make export latency measurable and materially faster without changing `.pen`
semantics or visual output. v0.6.1 is a focused renderer-performance release, not
a file-format revision or renderer rewrite.

Baseline: [`docs/render-benchmark-v0.6.0.md`](../../render-benchmark-v0.6.0.md).

## Release goals

- Make every export stage observable with stable machine-readable timings.
- Remove redundant validation, font preparation, and SVG processing.
- Improve large-document PNG latency while preserving pixel correctness.
- Keep memory bounded and report useful errors before oversized allocation.
- Establish CI regression gates based on relative baselines, not one developer's
  absolute machine timings.

## Must ship, in order

- [x] **1. Add a renderer benchmark harness.** Extend `pentool benchmark` or add
  `pentool benchmark render` with JSON output for:

  - document read and JSON parse;
  - validation;
  - SVG tree/string construction;
  - font database preparation and font resolution;
  - `usvg` parsing;
  - `resvg`/`tiny-skia` rasterization;
  - PNG encoding;
  - output write;
  - peak resident memory when the platform supports it.

  Support warmups, repetitions, median, p95, output scale, path-only versus text
  fixtures, and `--json`. Do not add overlapping stage values to a misleading
  `total`. Commit deterministic 100, 1,000, 10,000, and 100,000 object fixtures or
  generators, plus 1x, 2x, 4x, and 8x pixel-scale cases.

- [x] **2. Remove duplicated validation.** Introduce an internal validated export
  entry point so CLI/API boundaries validate exactly once. Public APIs must remain
  safe by default. Add tests proving invalid documents are still rejected through
  every public export path.

- [x] **3. Make font work proportional to fonts actually used.** Collect unique
  visible `(family, weight, style)` requests once per render. Query each unique face
  once, include only required bundled faces in SVG, and avoid decoding or loading
  the same embedded font multiple times. Cache immutable bundled font metadata and
  the system font database safely across exports. Missing-font errors must remain
  deterministic.

- [x] **4. Remove the PNG SVG-string round trip where practical.** Build one
  reusable render representation for SVG and PNG, or provide `usvg` with generated
  XML without redundant font data and allocations. Preserve ordinary SVG output,
  outlined-text output, IDs, stroke behavior, text layout, and embedded-font
  portability. Record the chosen architecture and rejected alternatives.

- [x] **5. Reduce SVG assembly allocations.** Pre-size the output buffer from
  document complexity, write numeric and escaped attributes into the buffer, avoid
  temporary `String`/`Vec` creation in hot loops, and share escaped/static values
  where safe. Optimize only after stage benchmarks identify material wins.

- [ ] **6. Profile rasterization and PNG encoding independently.** Use a sampling
  profiler on Windows, Linux, and macOS for object-heavy and pixel-heavy fixtures.
  Evaluate PNG compression-level controls, streaming/chunk behavior, and bounded
  parallelism. Defaults must favor interactive latency while remaining lossless and
  deterministic. Any new `--png-compression` or `--fast` option needs documented
  quality, size, CPU, and compatibility tradeoffs.

  Linux captures are committed for object-heavy and pixel-heavy runs under
  `docs/profiles/`. Windows and macOS sampling captures are still required.

- [x] **7. Add safe render limits and memory accounting.** Check scaled width,
  height, total pixels, row bytes, and estimated pixmap memory with checked
  arithmetic before allocation. Return structured errors for unsafe requests.
  Rendering must not panic or partially overwrite an existing destination.

- [x] **8. Add visual equivalence and regression tests.** Cover paths, fills,
  caps, joins, miters, transforms, multiline text, all bundled font faces, embedded
  fonts, transparent backgrounds, pages, and scales. Compare SVG structure where
  contractual and PNG pixels with an explicit tolerance. Store small golden files
  and produce diff artifacts on CI failure.

- [x] **9. Add performance gates.** On controlled CI runners, compare repeated
  medians against a checked-in baseline and fail only on statistically meaningful
  regressions. Track artifact size and peak memory alongside latency. Keep the
  existing functional cross-platform release matrix.

- [x] **10. Publish performance documentation.** Document the rendering stack,
  benchmark method, expected scaling, memory formula, CLI controls, and before/after
  results. Include raw machine-readable results and hardware/software metadata.

## Acceptance targets

Measured against v0.6.0 on the same machine, same release profile, same fixtures,
after warmup:

- 100,000-object scale-1 PNG median improves by at least **30%** (from 1,859 ms to
  **1,300 ms or less** on the baseline machine).
- 10,000-object scale-1 PNG median improves by at least **25%** (from 179 ms to
  **134 ms or less**).
- Text-document PNG exports improve by at least **20%** without increasing output
  size by more than 10% at the default compression setting.
- SVG export does not regress by more than **5%** at 10,000 and 100,000 objects.
- Peak render memory is measured and does not exceed **2.5x** the final RGBA pixmap
  plus document and encoded-output sizes for path-only fixtures.
- PNG and SVG conformance tests pass on Windows, Linux, Intel macOS, and Apple
  Silicon macOS.
- No network access, package resolution, or document mutation occurs during export.

## Stretch goals

- [ ] Reuse a parsed render tree for repeated exports of an unchanged document in
  the server/editor, with explicit invalidation after edits.
- [ ] Add cancellable progress between parse, raster, and encode stages.
- [ ] Investigate tile-parallel rasterization only if profiling shows it beats the
  simpler pipeline without seams or excessive memory.
- [ ] Evaluate an optional GPU backend behind a non-default experimental feature;
  do not make GPU availability a requirement.

## Out of scope

- Changing `.pen` format version or geometry semantics.
- Requiring a browser, GPU, cloud renderer, or network access.
- Trading away deterministic exports or font portability silently.
- Replacing `resvg`/`tiny-skia` before instrumentation proves that a replacement is
  necessary and conformance tests exist.

## Acceptance demo

Run the committed benchmark suite on v0.6.0 and v0.6.1 from the same clean machine.
Print per-stage JSON, median and p95 comparisons, peak memory, and output sizes.
Export the golden visual suite on all release platforms, verify SVG contracts and
PNG pixel diffs, then demonstrate a 100,000-object PNG export within the target
budget with no visual regression.
