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
