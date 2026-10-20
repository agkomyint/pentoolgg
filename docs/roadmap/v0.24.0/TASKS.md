# v0.24.0 — Agent previews and a faster CLI

Let an agent see what it just did, quickly and cheaply, and make every CLI call
cost less. Most agents are weak at checking visual results: today the loop is
mutate, export a full-size PNG in a second call, then read an image larger than
the model will ever look at.

## Baseline (measured 2026-10-20, Windows 11, release build without LTO)

| Step | Time |
| --- | ---: |
| Starting any process (`pentool --version`; `git --version` is the same) | ~75 ms |
| `info`, `tree --limit 5`, `search` on small and 55-layer documents | 90–100 ms |
| Export 55-layer blurred landscape, scale 1 | ~960 ms |
| Same, scale 0.3 | ~190 ms |
| First export after boot (cold font and file caches) | ~1500 ms |

So Pentool's own work for read commands is about 15–25 ms; process start is the
floor, and rendering at full size is where the time goes. The v0.23.0 layer
cache only lives inside the browser core: every CLI call renders cold, and
scene compositing and blur run on one thread.

## Product contract

- Previews never change the document, its revision or exported bytes. A
  preview is the normal CPU render at a smaller size, so the normative output
  path stays the only renderer.
- One call is enough to edit and see. A preview always matches the revision
  the same call wrote, and the result says which revision it shows.
- Small by default: preview images are sized for model input, not for print.
- Bounded: preview size, count, cache size and annotation count are limited
  and checked before work starts. Cache files are derived data, safe to delete,
  never needed to open a document, and never sent anywhere.
- Speedups are proven byte-identical against the previous release (the
  `hash-wasm` and native `compare-renders` checks used for v0.23.0) before they
  ship.
- Backward compatible: existing commands, flags and JSON fields keep their
  meaning; new fields are additive.

## Must ship, in order

### Agent previews

- [ ] **1. `--preview` on mutating commands** (`object`, `batch`, `shape`,
  `text`, `fill`, `effect`, `group`, ...). After the commit, render the touched
  page and add `"preview": {"path", "width", "height", "revision", "page"}` to
  the JSON result. Dry runs preview the planned result without writing the
  document. `PENTOOL_PREVIEW=1` turns it on for a whole session. Previews go to
  `.pentool/preview/` (bounded, oldest removed) unless `--preview-out` is given.
  *Implemented as global flags (`--preview`, `--preview-out`, `--preview-size`)
  with `tests/agent_preview.rs`; the box is ticked after three-OS CI.*
- [ ] **2. Size-bounded rendering.** A long-edge bound for previews
  (`--preview-size <px>`, default 1024, done with item 1) and `export`
  (`--max-size <px>`). It picks the scale, so agents stop guessing `--scale`.
  *`export --max-size` implemented with tests in `tests/agent_preview.rs`;
  the box is ticked after three-OS CI.*
- [ ] **3. Focus on the change.** `--focus <id>` renders the object's bounds plus
  padding at higher detail. With `--preview`, the default is the changed
  objects' union, plus a small full-page thumbnail for context.
- [ ] **4. `pentool preview DOC`** for read-only looks: page, `--focus`,
  `--max-size`, `--annotate`, `--compare <revision|file>`. Same renderer and
  JSON shape as item 1.
- [ ] **5. Labeled overlay (`--annotate`).** Draw object IDs and bounds on the
  preview (never on export), capped at N labels, with the label list in the
  JSON. Agents can then name what they see ("move `cloud-12` left") instead of
  guessing from `tree`.
- [ ] **6. Before/after numbers.** `--compare` reports `changed_pixels`,
  `changed_pct` and `changed_bbox`, plus an optional diff image. Often the agent
  can verify "only the right thing moved" without reading an image. Reuse the
  `analyze` and `paint-review` comparison code.

### Faster CLI

- [ ] **7. Persistent layer cache.** Store v0.23.0 node-cache entries in
  `.pentool/cache/` (content-addressed by the same sha256 key, bounded LRU,
  versioned, corrupt entries ignored). Edit-then-preview across separate CLI
  calls should then match the browser's ~8x. Enabled for previews and export;
  `--no-cache` and `PENTOOL_CACHE=0` turn it off.
- [ ] **8. One process for many commands.** `pentool session --jsonl` reads
  one command per line on stdin and writes one JSON result per line. It keeps
  the parsed document, fonts and layer cache warm, saving process start and
  parsing for every call after the first. It uses the same transaction path,
  locks and revision guards as single commands.
- [ ] **9. Multithreaded compositing and blur.** Split independent rows across
  `std::thread::scope` (no new dependency), as `photo/adjust.rs` already does.
  Results must be byte-identical whatever the thread count; `PENTOOL_THREADS=1`
  forces one thread.
- [ ] **10. Cold start.** Find what the first export after boot spends its
  1.5 s on (font discovery is the suspect) and make it lazy, or cache it in
  `.pentool/`.
- [ ] **11. Evidence.** A benchmark script for the agent loop (edit + preview,
  10 steps, small and 55-layer documents) recorded in `docs/performance.md`
  against v0.23.0 on all three OSes. Byte-identity checks for every speedup;
  tests for cache invalidation (edit, font change, linked file change, scale,
  page size) and for a corrupt or full cache.

## Targets

- Edit plus preview of one layer on the 55-layer landscape, in one call:
  under 150 ms warm, measured as v0.23.0 was.
- Read commands in a session: under 10 ms each after the first.
- No change to any exported byte.

## Explicitly deferred

- GPU previews (the `gpu` feature stays optional and non-normative).
- Video or animated previews.
- Sending previews to any network service.
- An image model that judges previews for the agent; Pentool supplies images
  and numbers, the agent does the judging.
