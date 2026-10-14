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

Release binaries are gated by `scripts/check-binary-size.sh` at 30,000,000 bytes in
CI and for every release target. The gate was raised from 24,000,000 bytes on
2026-10-10, by owner decision, when the developing v0.12 color engine took the
Windows release build to 24,247,808 bytes. Record the per-target sizes in the
release notes.

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

## Photography benchmark (v0.11)

`pentool benchmark --photo` qualifies the photo engine on a synthetic shoot
written to a temporary directory and removed afterwards:

```sh
pentool benchmark --photo --photos 24 --megapixels 2 --repetitions 3
pentool benchmark --photo --photos 1 --megapixels 24 --repetitions 3 --max-ms 120000
```

Each source is a 3:2 RGGB DNG of 16-bit uncompressed strips (12-bit values,
black 256, white 4095) with a smooth scene and deterministic per-photo texture.
The run:

1. writes the sources and ingests them by reference, as `raw add --external` does;
2. culls: rates every photo 2, every other one 4, and searches `rating>=4`;
3. develops the first photo at full size with tone, presence and a radial local
   adjustment, `--repetitions` times, and requires identical bits;
4. renders its Display P3 clipping preview twice through the preview cache, a
   miss and then a hit, and requires identical bytes;
5. exports the whole shoot with the `web-gallery` recipe.

The JSON report holds the fixture, `timings_us` (`generate`, `ingest`, `cull`,
`develop` per repetition, `preview` with each `cache` outcome, `export`), the
export file count and bytes, the develop and preview digests, and the peak
resident memory. `--max-ms` bounds the worst develop plus ingest, cull and
export. `--photos` is 1–500, `--megapixels` 0.01–50 and `--repetitions` 1–20;
`--scale` and `--warmups` are refused.

Measured on Windows 11 with an Intel Core i9-11900H, rustc 1.85.1, release
build, one developer machine (not a hosted-CI claim):

| Case | Result |
| --- | --- |
| 8 photos of 0.5 MP | develop 0.19–0.21 s; preview 247 ms cold, 11 ms from the cache; export of 8 files 1.73 s; peak 50 MB |
| 100 photos of 2 MP | generate 22 s; ingest 0.45 s; cull 3 ms; develop 0.69 s; export of 100 files 101 s (23 MB); peak 111 MB |
| 1 photo of 24 MP (6000x4000) | develop median 10–12 s (runs 9.5–16.6 s); preview 12.4 s cold, 26 ms from the cache; export 15.4 s; peak 1.12 GB |

A first 24 MP develop of 48.6 s was a single outlier that did not reproduce.
Memory grows with the size of the photo being developed, not with the size of
the shoot: 100 photos peak lower than a single 24 MP develop. Batch export is the
long pole of a large shoot at about 1 s per 2 MP photo, and a cached preview turns
a full develop into a file read.

## Developing v0.12 working-color precision

On 2026-10-10, the shared byte blend kernel was extracted for reuse by bounded
float linear RGB surfaces. Ordinary scene exports still use the byte renderer;
this comparison qualifies that extraction, not completed float scene rendering.
Both builds were optimized on Windows 11, Intel Core i9-11900H (8 cores/16
threads), rustc 1.85.1, LLVM 19.1.7, `x86_64-pc-windows-msvc`.

The comparison ran three alternating invocations per binary after tests and
compilation finished, using:

```sh
pentool benchmark --composite --source-size 256 --clipped 16 --stack-depth 8 --blur-radius 32 --repetitions 3 --json
```

Each invocation produced one cold and three warm samples (12 per binary).
Every sample produced the same PNG SHA-256:
`3ae354e380b34064e7edebd8368281efb6929cb58d3dee85cb395acf43d48055`.

| Build | Median | Range | Peak resident range | Binary bytes |
| --- | ---: | ---: | ---: | ---: |
| Native gradient checkpoint | 662.7 ms | 614.2–747.9 ms | 27.2–40.9 MB | 24,236,032 |
| Shared byte/float blend kernel | 620.4 ms | 543.9–664.8 ms | 27.3–27.6 MB | 24,236,032 |

The overlapping ranges do not establish a general speedup. The measured median
ratio was 0.936; output bytes and binary size were unchanged. These are local
development observations, not hosted or cross-platform qualification.
The Windows binaries exceeded the 24,000,000-byte release gate in force at the
time; the gate is now 30,000,000 bytes (see above).

Exact binary fingerprints:

- Before: `3719748c3963287b0ef09496368f544b1ad348a463e917c6c79b963c77cf9b23`.
- After: `2edf2cfdc39af6954381542f98df417dedc4c74059a56c1f3cc5d1fecdba5aa0`.

Float surfaces separately enforce a 256 MiB backing-allocation limit, finite
signed RGB, normalized coverage, matching dimensions and working primaries,
and validation before mutating a destination. Scene integration must account
for their actual backing capacity and aggregate live scratch; the current byte
renderer budget must not be reused as if a float pixel occupied four bytes.

### Shared fill sampling targets

On the same Windows/Rust machine, the existing fill geometry loop was extended
with a float working-color target. An optimized before/after comparison exported
the pinned `print-v8-gradient.pen` at scale 8 (512x384), including its linear RGB,
Lab and authored CMYK ramps. Five alternating invocations per binary followed
one warm-up invocation per binary, after compilation and tests finished:

```sh
pentool export docs/fixtures/print-v8-gradient.pen OUTPUT.png --scale 8 --json
```

| Build | Median CLI export | Range | Binary bytes |
| --- | ---: | ---: | ---: |
| Float surface foundation | 390.8 ms | 333.7–423.0 ms | 24,236,032 |
| Shared byte/float fill loop | 385.1 ms | 365.4–400.5 ms | 24,247,808 |

These end-to-end timings include startup, validation, rendering, PNG encoding
and file writing. Their overlapping ranges do not establish a speedup. Every
output, including warm-ups, had PNG SHA-256
`d3d37d5c84b9d85fce710d597b926f6ff020d1d93e19712b0015dc5dd070e287`.
This checks the existing export path, not completed float scene performance.
The binary increased by 11,776 bytes. It exceeded the 24,000,000-byte gate in
force at the time and is within the current 30,000,000-byte gate.

Binary SHA-256 before:
`2edf2cfdc39af6954381542f98df417dedc4c74059a56c1f3cc5d1fecdba5aa0`;
after:
`6624e3e04c073919cd5cfbee480366ebf98565653354f1626d15a4b48f076c73`.

## Long-document layout (0.14)

500 one-page spreads, one two-column text frame per page (540x720 pt), a single
threaded story of 3,500 paragraphs, `h1`/`body` styles. Release build, Windows 11,
one run each, wall time of the CLI process including load and write:

| Operation | 125 pages | 500 pages |
| --- | --- | --- |
| `layout-preflight` (compose and measure) | 0.23 s | 0.64 s |
| `layout-export` EPUB | 0.19 s | 0.58 s |
| `idml-export` | 0.04 s | 0.08 s |
| `layout-export` tagged PDF | 6.7 s | 28.8 s (7 MB) |

Composition and the structured exports scale linearly and stay under a second.
The tagged PDF dominates because each page is rasterized as a background image
beneath the real text; it was 43 s before page rasters stopped round-tripping
through PNG. Skipping rasterization of pages with no non-text content is the
next candidate. Reproduce with a generated document of this shape; the numbers
are one-machine measurements, not a guarantee.

## Static 3D reference-renderer benchmark (v0.15)

The release qualification probe isolates bounded triangle traversal and raster
work by drawing one full-frame triangle 100,000 times with shared vertices. It
uses the normative CPU renderer at 256x256, one sample per pixel, one light and
no shadow map. An optional square texture exercises sampling and the decoded
texture budget. The command reports every sample, its minimum/median/maximum,
the output digest and peak resident memory:

```sh
pentool benchmark --scene3d --triangles 100000 --repetitions 3 --max-ms 30000
pentool benchmark --scene3d --triangles 100000 --texture-edge 4096 --repetitions 3 --max-ms 30000
```

The 30-second ceiling is the initial release budget for one untextured sample;
CI runs one sample against it on every release target. It is intentionally above
the developer baseline so shared hosted runners do not turn ordinary variance
into failures. Triangle count is limited by the importer budget, texture memory
uses the same decoded-byte limit, and repetitions are limited to 20.

The initial untextured baseline was measured on 2026-10-14 with an optimized
x86_64-pc-windows-msvc build on Windows 11 Pro 10.0.26200, an Intel Core
i9-11900H (8 cores/16 threads), rustc 1.99.0 and LLVM 23.1.1. Three samples were
10.108, 10.421 and 10.475 seconds (10.421-second median), peak resident memory
was 33,103,872 bytes, and every run produced pixel digest
`sha256:afe972e005866dbef2d148ba2e34ba9a52fece4e8c477d22ea3297a67732635b`.
The default release binary was 42,787,328 bytes. These are local observations;
the CI matrix remains the cross-platform performance authority.
