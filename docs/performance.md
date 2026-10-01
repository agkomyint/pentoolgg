# Performance and document limits

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
