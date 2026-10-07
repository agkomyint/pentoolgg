# Performance and document limits

The v0.6.0 renderer baseline and v0.6.1 optimization plan are documented in
[`render-benchmark-v0.6.0.md`](render-benchmark-v0.6.0.md) and
[`roadmap/v0.6.1/TASKS.md`](roadmap/v0.6.1/TASKS.md).

Pentool validates documents before editing or rendering. Current hard limits are
1,000 pages, 1,000 layers per page, 100,000 paths, 100,000 text objects, 16,384
canvas units per axis, 64 embedded fonts, and 16 MiB total base64 font data. CLI
imports reject input files larger than 64 MiB; the browser API uses the same body
limit.

Use the built-in repeatable benchmark on the machine that will run Pentool:

```sh
pentool benchmark --layers 1000 --objects 100000
pentool benchmark --layers 100 --objects 10000 --png --max-ms 60000
```

The command reports JSON byte size and separate generation, serialization, parse,
validation, index, paginated search, targeted edit, SVG, and optional PNG times.
`--max-ms` exits unsuccessfully when the total exceeds the chosen release budget.
Results depend on CPU, fonts, path complexity, canvas size, and output scale; they
are qualification measurements rather than universal latency guarantees.

Search output defaults to 100 objects and accepts `--offset` and `--limit` (maximum
10,000). The browser displays at most 500 matching object rows and uses debounced
search plus CSS content visibility. Refine the search to inspect additional rows.
Browser import and PNG export expose cancellation; CLI jobs can be interrupted
without modifying the destination because imports write only after validation.

Committed CLI imports use a sibling temporary file, retain the exact prior file as
`<name>.bak.N`, and restore it if final replacement fails. Dry runs never write.

## Raster image benchmarks and distribution budget (v0.7)

## Development compositing qualification (v6 / v0.8 roadmap)

```sh
pentool benchmark --composite --source-size 256 --clipped 16 --stack-depth 8 --blur-radius 32 --repetitions 3
pentool benchmark --composite --source-size 512 --clipped 32 --stack-depth 16 --blur-radius 256 --repetitions 3
```

The fixture reuses one processed image, one mask resource across clipped nodes,
deep isolated groups, a reversible grade and a large-radius blur. A fresh private
cache measures the first cold run; subsequent warm runs must produce identical
PNG hashes. JSON records the fixture, timings, output size and peak resident
memory where supported. The temporary directory is removed after measurement.

The compositor limits temporary surfaces to 256 MiB and cumulative work to
1,073,741,824 pixels, including conservative mask/blur scratch and retained scope
coverage. Source decoding retains its separate v5 limits; total resident memory
must therefore be measured, not inferred from the compositor scratch limit.
Stacks are limited to 64 transforms, 32 effects and 64 adjustments per page;
resources/names to 256 masks; selection queries to 128 expressions/nesting 16.
Canvas axes are at most 16384. Large requests fail instead of degrading output.
Hosted CI on all four release targets remains required for qualification.

Windows development qualification on 2026-10-06 used an Intel Core i9-11900H,
Rust 1.85.1, and an optimized x86_64-pc-windows-msvc build. These measurements
are reproducible observations, not release budgets:

| Fixture | Cold | Warm range | Peak RSS | Output |
|---|---:|---:|---:|---:|
| 256 px, 16 clips, depth 8, blur 32 | 755 ms | 680–787 ms | 38.7 MB | 31,495 B |
| 512 px, 32 clips, depth 16, blur 256 | 6.56 s | 7.80–9.35 s | 53.9 MB | 20,395 B |

Every cold/warm run within each fixture produced the same PNG SHA-256. The
release binary measured 16,342,016 bytes after the compositing implementation.
Hosted results and platform-specific binary sizes must be recorded before release.

### Existing raster image benchmark

```sh
pentool benchmark --images 100 --source-size 512 --operations 2 --repetitions 5
```

One PNG source is reused `--images` times, each with an operation stack of
`--operations` entries (0–8). The report lists codec, source dimensions, reuse
count, document size, and one record per run: `cold` (empty processed cache) then
`warm`, with decode/process/compose, rasterize/encode, and write times in
microseconds, output bytes, and peak resident memory where the platform reports it.
Output bytes must be identical across cold and warm runs.

The editor requests approximate previews with `/api/render/svg?max_edge=N`
(16–4096). Proxies reduce embedded pixel detail only; exports never use them.

Release binaries are gated by `scripts/check-binary-size.sh` at 24,000,000 bytes in
CI and for every release target. The Windows release build measured 14,630,400
bytes at the time of writing; record the per-target sizes in the release notes.

## Raster paint benchmarks (v0.10)

`tests/raster_perf.rs` holds ignored benchmarks; run them on a release build:

```sh
cargo test --release --test raster_perf -- --ignored --nocapture
```

They call the in-process engine (`raster::apply`), so each stroke includes the full
validated copy-on-write document update but no file I/O. Measured on Windows 11, one
developer machine, release build, 40-sample strokes with pressure:

| Case | Result |
| --- | --- |
| Stroke latency, 24 px brush, 1000x1000 / 4000x5000 / 8000x8000 layers | p50 5.7-7.3 ms, p95 9.6-10.8 ms; at most 4 tiles dirtied per stroke; layer size does not change latency |
| 10,000 strokes on a 1024x1024 layer | 86 s total (about 4.6 ms per stroke at the start, 10 ms near the end; 146 s before a stroke stopped re-hashing every tile); journal held at 16 entries by checkpoints; `verify --replay` 79 ms; replay 51 ms; document 0.1 MiB |
| Same strokes on a 4000x5000 layer spread over the canvas (measured before the re-hashing change) | 500 strokes 9 s, 1500 strokes 47 s, 2500 strokes 111 s |
| Compositing 60 large strokes, 2000x2000 page | 1.5 s |

Known limit: the per-stroke cost of a long editing session grows with the number of
strokes already painted (the second row), and grows faster when strokes touch many
distinct tiles (the third). Reopening is not affected: the bounded checkpoint plus
journal keeps replay under 100 ms. What remains is decoding every stored tile of the layer on each stroke; loading only the tiles a stroke touches is future work;
these numbers are not a hosted-CI performance claim.

Correctness hardening lives in `tests/raster_hardening.rs`: seeded fuzzers for sample
streams, brush JSON, selection geometry, journal and tile corruption, and brush-asset
imports; plus seam, soft-alpha/pressure and replay tests. The first run of the selection
fuzzer found integer overflows in marquee and lasso bounds, now fixed.
