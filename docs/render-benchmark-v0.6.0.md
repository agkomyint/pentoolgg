# Pentool v0.6.0 renderer baseline

Measured on 2026-10-03 with the release build of Pentool 0.6.0.

## Test machine

- Windows, PowerShell 7.6.6
- Intel Core i9-11900H, 8 cores / 16 logical processors
- 16 GB RAM
- `pentool 0.6.0`, release profile with LTO and one codegen unit

These are machine-local qualification numbers, not universal performance claims.
Each large-document result is the median of five runs. Each CLI export result is
the median of 12 fresh-process runs and therefore includes process startup,
document I/O, JSON parsing, validation, rendering, output encoding, and output I/O.

## Generated large-document benchmark

The built-in benchmark creates simple paths with text every tenth object on a
1200 x 800 canvas. `png` is the complete in-memory `to_png` call at scale 1 and
includes its own SVG generation, SVG parsing, font setup, rasterization, and PNG
encoding. The separately reported `svg` value is an additional `to_svg` call.

| Objects | Layers | JSON | SVG | SVG median | PNG median | PNG / SVG |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 10 | 20 KB | 302 KB | <1 ms | 5 ms | n/a |
| 1,000 | 20 | 191 KB | 438 KB | 1 ms | 37 ms | 37x |
| 10,000 | 100 | 1.87 MB | 1.76 MB | 15 ms | 179 ms | 11.9x |
| 50,000 | 500 | 9.37 MB | 7.69 MB | 87 ms | 945 ms | 10.9x |
| 100,000 | 1,000 | 18.76 MB | 15.09 MB | 169 ms | 1,859 ms | 11.0x |

Both object-heavy stages scale approximately linearly in this test. SVG string
generation is not the primary bottleneck. PNG conversion dominates and accounts
for roughly 73% of the built-in benchmark's median end-to-end time at 100,000
objects.

## Real document CLI latency

| Document | Format | Median | Minimum | p95 sample |
| --- | ---: | ---: | ---: | ---: |
| `x-logo-concept.pen` | SVG | 15.35 ms | 13.61 ms | 18.71 ms |
| `x-logo-concept.pen` | PNG | 21.21 ms | 17.73 ms | 33.84 ms |
| `examples/text-demo.pen` | SVG | 26.68 ms | 23.22 ms | 33.92 ms |
| `examples/text-demo.pen` | PNG | 47.22 ms | 43.04 ms | 51.02 ms |
| `examples/multipage-ai-chat.pen` | SVG | 26.68 ms | 16.76 ms | 27.85 ms |
| `examples/multipage-ai-chat.pen` | PNG | 50.30 ms | 45.17 ms | 59.56 ms |

The p95 column is the second-slowest of 12 samples, not a statistically robust
production p95. It is retained as a regression signal.

## Resolution scaling

The path-only X logo was exported repeatedly at increasing PNG scale.

| Scale | Output pixels | Median |
| ---: | ---: | ---: |
| 1x | 768 x 768 | 17.18 ms |
| 2x | 1536 x 1536 | 24.27 ms |
| 4x | 3072 x 3072 | 48.21 ms |
| 8x | 6144 x 6144 | 134.42 ms |

The increasing pixel cost points to rasterization and PNG encoding as the main
resolution-sensitive work. Object count and pixel count must remain separate axes
in future benchmarks.

## Current rendering stack

1. `serde_json` reads the `.pen` document into the Rust `Document` model.
2. Pentool validation checks document and object limits.
3. `render::to_svg` builds an SVG string and embeds font faces when text exists.
4. SVG output writes that string directly, unless text outlining is requested.
5. PNG output parses the generated SVG with `usvg` from `resvg 0.44`.
6. `resvg` rasterizes into a `tiny-skia 0.11` pixmap on the CPU.
7. `tiny-skia` encodes the pixmap as PNG and Pentool writes it to disk.

No browser, GPU, canvas API, or network service is involved.

## Bottlenecks and avoidable work

### Confirmed by measurements

- PNG conversion dominates object-heavy exports: 1,859 ms versus 169 ms for the
  separately measured SVG build at 100,000 objects.
- PNG time grows with output pixel count even when document complexity is fixed.
- Text documents have meaningful fixed startup cost compared with a path-only
  document.

### Confirmed by code inspection

- CLI export validates in `read_document`, then `to_svg` validates the same
  document again.
- `to_png` first creates a complete SVG string and then reparses it into a `usvg`
  tree, producing a large temporary allocation for complex documents.
- Every text SVG embeds all four bundled Atkinson Hyperlegible font files even if
  only one face is used.
- PNG setup constructs a font database, copies bundled font bytes, clones system
  font face metadata, and queries the database once for every visible text object.
- Font data is supplied both through SVG `@font-face` data URLs and the `usvg`
  options database during PNG export.
- SVG assembly performs many short `format!` calls and escaped-string allocations.
- The built-in benchmark reports only aggregate PNG time. It cannot distinguish
  SVG construction, `usvg` parsing, rasterization, encoding, and file I/O, and its
  `total` includes both a standalone SVG build and the SVG build inside PNG.

The first optimization target should be redundant validation/font/SVG work. It is
lower risk than replacing the rasterizer and preserves output compatibility.

## Reproduction

```powershell
.\target\release\pentool.exe benchmark --layers 100 --objects 10000 --png
.\target\release\pentool.exe benchmark --layers 500 --objects 50000 --png
.\target\release\pentool.exe benchmark --layers 1000 --objects 100000 --png
.\target\release\pentool.exe export x-logo-concept.pen output.png --scale 1
.\target\release\pentool.exe export x-logo-concept.pen output.svg
```

Use multiple runs, report median and tail samples, close CPU-heavy applications,
and compare release builds on the same machine.
