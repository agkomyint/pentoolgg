# Pentool v0.7.0 — bug report

**Status:** all six items are fixed in 0.7.1 (see `tests/v071_fixes.rs`). Image-scene PDFs remain raster pages; text now appears in them.

Tested on: `pentool 0.7.0` (x86_64-unknown-linux-gnu, SHA-256 verified against `SHA256SUMS.txt`), compared against 0.6.24.
Date: 2026-10-06. Image ops, import, bake, undo, dry-run, if-revision, linked SVG and error codes were exercised and work; the items below are what failed.

| # | Severity | Summary | Status vs 0.6.24 |
|---|----------|---------|------------------|
| 1 | **High** | Text vanishes from PNG and PDF export once the document contains any image node | New in 0.7.0 |
| 2 | Medium | `document has no canvas` on `layer add`, `page list/add`, `text put` after `new` + `canvas` | Still open |
| 3 | Medium | `batch put-*` cannot replace an existing ID (no upsert) | Still open |
| 4 | Low | `tree` does not nest group children | Still open |
| 5 | Low | Missing-file error has no code and no path | New |
| 6 | Low | `image add --help` documents no `--mask`; docs say `--mask` is an `add` option | Docs mismatch |

---

## 1. Text disappears from PNG/PDF export when an image is present (HIGH)

**Expected:** text renders regardless of other node kinds.
**Actual:** every `text` node disappears from the PNG and PDF export as soon as the document holds any image node. The text is still in the `.pen` and still in the SVG export, so the fault is in the PNG/PDF rasterizing path, not in the document.

### Minimal repro
Needs `pentool` on PATH (or `PENTOOL=/path/to/pentool`), Python 3 with Pillow.
`repro.sh` (attached next to this file) does everything below.

```sh
pentool new bug.pen --width 400 --height 160
pentool canvas bug.pen --background "#222222"
pentool batch bug.pen ops.json        # one put-text "HELLO", size 72, white
pentool export bug.pen before.png     # text visible
pentool image add bug.pen pic --file tiny.png --layer layer-1 --x 300 --y 20 --width 80 --height 80 --embed
pentool export bug.pen after.png      # text GONE
pentool export bug.pen after.svg      # text still present
```

Measured (white pixels in the text region): **before image: 5103 → after image: 0.**
Removing the image (`image remove`) brings the text back.

### Narrowing (all measured on 0.7.0)
- **Only text is affected.** In one document, rect and path nodes kept all their pixels (12000 and 3960 px both unchanged); text went 2472 → 0.
- **Any image triggers it:** embedded or `--external`; a normal photo or a fully transparent 64×64 PNG.
- **Order does not matter:** image added before or after the text gives the same result.
- **Font does not matter:** Poppins, Atkinson Hyperlegible and the default font all vanish.
- **PDF is affected too:** `pdffonts` lists 1 font without an image and 0 with one; `pdftotext` finds no text.
- **SVG export is fine** (`<text>` present) and renders correctly in Chromium, which is the workaround I used.
- 0.6.24 has no image support, so this is a regression introduced with the image pipeline (suspect: the render path that loads/decodes images skips or replaces the font database).

### Workaround
`pentool export doc.pen doc.svg`, then rasterize the SVG with Chromium/Playwright.

---

## 2. `document has no canvas` after `new` + `canvas` (MEDIUM, carried over)

Same family as the v4 bug in 0.6.24, now on v5 files.

```sh
pentool new n.pen --width 600 --height 240
pentool canvas n.pen --background "#222"
pentool layer n.pen add L2                                              # Error: document has no canvas
pentool page n.pen list                                                 # Error: invalid .pen document: document has no canvas
pentool page n.pen add p2                                               # Error: document has no canvas
pentool text n.pen put t1 --layer layer-1 --content hi --x 10 --y 10    # Error: document has no canvas
```

`shape`, `path put` and `batch` work on the same file, so the failure is specific to these commands' document loader. Not retested on this build: `object set/remove`, `geometry`, `layer-geometry`, `add --mode instance` (all failed this way on 0.6.24).
**Workaround:** use `batch` with `put-text` / `put-shape` / `put-path`.

## 3. `batch put-*` cannot replace an existing ID (MEDIUM, carried over)

```sh
echo '[{"type":"put-shape","shape":"rect","id":"u","layer":"layer-1","x":1,"y":1,"width":5,"height":5,"fill":"#fff"}]' > u.json
pentool batch n.pen u.json      # ok
pentool batch n.pen u.json      # Error: ... node ID already exists on page: u
```

`put` suggests create-or-replace; for agents this means re-running the same batch fails, and every change needs a separate `object set`. An `--upsert` flag or `"mode":"replace"` would fix it. (Same for `put-text`, hit while re-sending a label batch.)

## 4. `tree` does not nest group children (LOW, carried over)

After `create-group` with `children: ["u"]`, `tree --json` lists `gg` and `u` as siblings; `gg` has no `children` key. Group membership is invisible to agents reading the tree.

## 5. Missing-file error has no code or path (LOW)

```sh
pentool image add g.pen missing --file nope.png --layer layer-1 --x 0 --y 0 --width 10 --height 10
# Error: No such file or directory (os error 2)
```

Other image errors carry codes (`[malformed-resource]`, `[invalid-operation]`, `[unsupported-capability]`, `[unsafe-path]`). This one should be `[missing-resource]` and name the path.

## 6. Docs mismatch for `--mask` (LOW)

`docs/image-workflow.md` shows `--mask SHAPE_ID` under crop/fit/mask semantics next to `image add`, but `image add --mask` is rejected (`unexpected argument '--mask'`). It works on `image set` (`image set g.pen j --mask m`). Fix the docs or accept the flag on `add`.

---

## What works (so nobody re-tests it)
- Import PNG, JPEG and WebP; dedup by `sha256`; bad file → `[malformed-resource]`.
- Op stack (grayscale, blur, brightness-contrast, hue-saturation, sharpen, curves, rotate…) with range checks: `rotate 45` and `blur 9999` are rejected with `[invalid-operation]`; unknown kind with `[unsupported-capability]`.
- `image op disable`, `bake` (single history entry, undoable, `--dry-run` predicts the hash), `palette` (creates `brand-N` tokens), `analyze`.
- `--dry-run` and `--if-revision` (stale revision is rejected and the file is untouched).
- Crop validation (`2 2 2 2` → `[malformed-resource]`); `image set` crop, fit, opacity and mask.
- Determinism: the same document exported twice is byte-identical.
- Linked SVG fails closed when an image has ops or crop, with a clear message.
- `migrate` to v4 refuses documents holding images: `[unsupported-capability] version 4 cannot represent image assets`.

## Not bugs (my test mistakes, listed to save time)
- `export` to `.jpg`/`.webp` is unsupported (PNG, SVG, PDF only; same on 0.6.24).
- `--crop` takes four space-separated values, not a comma list.
## v0.7.1 retest follow-up (fixed in 0.7.2)
- Font weight, italic, alignment, letter spacing and line height collapsed to defaults once a document held an image: the image scene writer now emits the same typography attributes as the non-image renderer.
- PDF export of image scenes dropped text: pages now carry an invisible text layer (render mode 3, Helvetica) so the text is selectable and searchable. Positions ignore group transforms, and characters outside Latin-1 become `?` in the extractable text.
- A single missing glyph (for example `→`) switched the whole line to a bold serif fallback: start-aligned lines now isolate symbol runs into their own text chunk, and the fallback face is chosen to match the requested weight and style.
- `batch --upsert` / `"mode":"replace"` and `--mask` (an `image set` flag, not `image add`) behave as documented since 0.7.1.
