# Pentool v0.6.1 renderer benchmark

The stage-level harness measures a single PNG export pipeline without overlapping
stage values. Fixture generation and warmups are excluded from `total`.

```powershell
pentool benchmark --render --layers 100 --objects 10000 `
  --warmups 2 --repetitions 7 --scale 1 --json
pentool benchmark --render --layers 1000 --objects 100000 `
  --paths-only --warmups 2 --repetitions 7 --scale 1 --json
```

Use `--objects` with 100, 1000, 10000, or 100000 and `--scale` with 1, 2,
4, or 8 for the release matrix. The deterministic generator supports mixed text
and path fixtures by default; `--paths-only` isolates path rendering.

The JSON report contains every raw sample plus median and p95 values in
microseconds for document read/parse, validation, SVG construction, font
preparation/resolution, `usvg` parsing, rasterization, PNG encoding, output write,
and non-overlapping total time. It also reports encoded size, estimated RGBA
pixmap bytes, and peak resident memory on Windows and Linux.

Run release builds on the same otherwise-idle machine for comparisons:

```powershell
cargo build --release
.\target\release\pentool.exe benchmark --render --layers 1000 `
  --objects 100000 --paths-only --warmups 2 --repetitions 7 --json
```

Rendering rejects requests above 268,435,456 pixels or 1 GiB of estimated RGBA
pixmap memory before allocation. Width, height, row bytes, pixel count, and pixmap
bytes use checked arithmetic. Exports render completely to memory and use a
same-directory temporary file, preserving an existing destination if rendering or
replacement fails.

## Rendering architecture

The public SVG path validates once, collects the visible font requests, embeds
only the required bundled faces plus document fonts, and assembles portable XML.
The PNG path validates once and builds the same XML geometry without base64 font
payloads. It supplies fonts through `usvg::Options::fontdb`, parses the compact
XML once, rasterizes into one RGBA pixmap, and encodes that pixmap with
`tiny-skia`'s deterministic lossless PNG encoder.

This retains an XML intermediate because `usvg`'s public API accepts SVG XML and
owns its render tree. Constructing `usvg` internals directly would couple Pentool
to non-public representation details. A browser renderer and a replacement raster
backend were rejected because they increase deployment complexity and would change
pixel behavior. Retaining embedded data URLs in the internal PNG XML was rejected:
the same fonts are already present in the native database and the payload causes
avoidable allocation and parsing.

SVG assembly estimates capacity from visible object count and content bytes, then
writes numbers and escaped values directly into one buffer. Bundled and system
font metadata are cached process-wide; embedded-font validation has a bounded
eight-entry cache. Face availability is queried once per unique visible
`(family, weight, italic)` tuple.

## Local results

Measured on 2026-10-03 on the Windows baseline machine described in
[`render-benchmark-v0.6.0.md`](render-benchmark-v0.6.0.md), using a release build,
two warmups, and five repetitions:

| Fixture | End-to-end median | Render-only stage sum | SVG assembly | `usvg` parse | Raster | PNG encode | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 mixed | 172.9 ms | 152.4 ms | 2.7 ms | 91.2 ms | 55.8 ms | 2.6 ms | 48.5 MB |
| 100,000 paths | 560.9 ms | 399.1 ms | 19.1 ms | 292.6 ms | 85.4 ms | 2.0 ms | 244.6 MB |
| 100,000 mixed | 1,743.0 ms | 1,518.1 ms | 24.8 ms | 909.9 ms | 580.1 ms | 2.4 ms | 358.6 MB |

Raw reports: [`10,000 mixed`](render-benchmark-v0.6.1-10000.json),
[`100,000 paths`](render-benchmark-v0.6.1-100000-paths.json), and
[`100,000 mixed`](render-benchmark-v0.6.1-100000.json).

The v0.6.0 aggregate in-memory PNG medians were 179 ms and 1,859 ms for the mixed
10,000- and 100,000-object fixtures. The comparable v0.6.1 render-stage sums are
about 15% and 18% lower. These results are improvements, but do **not** meet the
roadmap's 25% and 30% acceptance targets. `usvg` parsing and text rasterization
remain the dominant work. The path-only peak-RSS target is also not yet met because
the parsed SVG tree is much larger than the final pixmap.

## PNG encoding and profiling decision

Stage timings show PNG encoding at roughly 2–3 ms at scale 1, while `usvg` parsing
and rasterization dominate. `tiny-skia` 0.11 exposes a deterministic
`encode_png()` operation but no stable compression-level or streaming control.
Adding another encoder solely for a low-single-digit-millisecond stage would add
dependency and compatibility risk. No `--fast` or `--png-compression` option is
introduced in v0.6.1. Rasterization remains single-threaded; document-level
parallel exports can be bounded by callers without introducing tile seams.

For sampling profiles, use the same object-heavy and pixel-heavy commands on each
release platform and attach the profiler output to the release qualification run:

- Windows: Windows Performance Recorder/Analyzer or Visual Studio CPU Usage.
- Linux: `perf record --call-graph dwarf -- <command>` followed by `perf report`.
- macOS: Instruments Time Profiler with the release binary and arguments above.

Cross-platform sampling captures are still required before roadmap item 6 can be
marked complete. Linux/WSL2 captures made with `samply 0.13.1` at 1,000 Hz are
checked in as [`object-heavy`](profiles/linux-object-heavy.json.gz) and
[`pixel-heavy`](profiles/linux-pixel-heavy.json.gz), with their corresponding
`.syms.json` symbol sidecars. The optimized profiling build retained debug symbols
and frame pointers; `perf_event_paranoid` was temporarily changed from 2 to 1 for
capture and restored to 2 afterward.

The Linux stage measurements agree with the sampling focus: the 100,000-path run
spent a median 284.6 ms in `usvg` parsing, 68.7 ms rasterizing, and 1.9 ms encoding.
The 8x pixel-heavy run reversed that balance: 123.9 ms encoding, 33.3 ms
rasterizing, and 0.2 ms parsing. This confirms that compression controls could
matter for very large pixel outputs but would not materially improve ordinary
scale-1 object-heavy exports.

## Regression gate

The manually dispatched `Controlled renderer performance` workflow runs only on a
self-hosted runner labeled `renderer-benchmark`. It compares seven-run medians to
the checked-in 100,000-path report and rejects regressions above 15% in total, SVG
assembly, parsing, rasterization, or encoding. It separately limits PNG size growth
to 10% and peak-memory growth to 15%, and uploads the current raw JSON.
