# Pentool — all open bugs (verified on v0.8.0)

Tested: `pentool 0.8.0` (x86_64-unknown-linux-gnu, SHA-256 verified), 2026-10-07. No newer release existed (v0.8.1 / v0.8.2 / v0.9.0 all 404).

**How to use this:** `repro_all.sh` (same folder) runs one check per bug below and prints `PASS` (fixed) or `FAIL` (still present). It uses a throwaway temp dir.
```sh
PENTOOL=/path/to/pentool ./repro_all.sh      # needs python3 + Pillow, pdftotext/pdffonts
```
Current result on 0.8.0: **14 of 15 checks fail** (only B04a passes). Patch, re-run, watch it go green.

## Summary

| ID | Sev | Bug | First seen |
|----|-----|-----|-----------|
| B01 | **High** | Design-token changes are ignored when the document contains an image (PNG **and** SVG) | 0.7.2 |
| B02 | Med | `batch put-*` cannot replace an existing ID (no upsert) | 0.6.x |
| B03 | Low | `image add --mask` rejected; `docs/image-workflow.md` says it exists | 0.7.0 |
| B04 | Med | PDF text is not extractable on **v6** documents (v5 is fine) | 0.8.0 |
| B05 | **High** | Adjustment cost scales with canvas area, not the target node's size | 0.8.0 |
| B06 | **High** | `outer-glow` (and other effects) disappear when the image has a mask | 0.8.0 |
| B07 | Med | `preset paste` silently removes the target's mask | 0.8.0 |
| B08 | Med | Gradient fills: no coordinates → flat end colour; out-of-node geometry accepted silently | 0.8.0 |
| B09 | Low | `linked report` prints the whole embedded image as base64 | 0.8.0 |
| B10 | Low | `effect add --kind gradient-overlay` unusable from the CLI (`fill kind missing`) | 0.8.0 |
| B11 | Low | Misleading error: `preset save ... --name` → "does not mutate the document" | 0.8.0 |
| B12 | Low | `effect list` needs an `EFFECT_ID`; there is no way to list a node's effects | 0.8.0 |
| B13 | Med (docs) | `--params` keys and enums are undocumented; discoverable only by trial and error | 0.8.0 |
| B14 | Med (perf) | v6 documents export ~3x slower than v5 even with no adjustments | 0.8.0 |

---

## B01 — Token changes ignored when an image is present  (HIGH)
`repro_all.sh` ids: B01a (v5 doc), B01b (v6 doc). Both fail.

```sh
pentool new b.pen --width 300 --height 150 && pentool canvas b.pen --background "#101010"
# one batch: token color.a=#FF0000 and a rect bound to it via fill_ref
pentool batch b.pen j.json
pentool image add b.pen im --file photo.png --layer layer-1 --x 150 --y 10 --width 100 --height 60 --embed
# change the token to blue
echo '[{"type":"set-style","name":"color.a","style_type":"color","value":"#0000FF"}]' > c.json && pentool batch b.pen c.json
pentool export b.pen b.png       # rect is RED (255,0,0); expected BLUE (0,0,255)
pentool export b.pen b.svg       # also stale: fill="#FF0000"
```
- Same document **without** the image renders blue (token followed). Shape on a second layer / moved layer also fine. Only the presence of an image node triggers it.
- `style list` shows the new value and `style usage color.a` still lists the shape, so storage and binding are correct; render preparation ignores the token (uses the stored fallback).
- SVG export is stale too, so it is not the PNG rasterizer; it is the shared "prepare for render" step.
- **Suspect:** two render-preparation paths (image-containing docs vs plain docs). Image docs skip token resolution. Same family as 0.7.0 (text vanished), 0.7.1 (weights collapsed), 0.7.1 (PDF text lost), all fixed individually; this one wasn't.
- **Expected:** token resolution identical with or without images. Please add a regression test: set token → export → pixel check, with and without an image.
- Related: text nodes that mix flat `fill` with nested `style.fill.ref` silently lose the ref (stored as fallback only). Docs say not to mix; the CLI should reject it.

## B02 — No upsert in `batch`  (MED)
`repro_all.sh`: B02.
```sh
pentool batch u.pen u.json   # put-shape id=u: ok
pentool batch u.pen u.json   # Error: ... node ID already exists on page: u
```
Re-running the same batch fails, so agents can't "apply this design" idempotently; they need a separate `object set` per node. Suggest `"mode":"replace"` per op or `batch --upsert`. Applies to put-shape, put-text, put-path, put-image.

## B03 — `image add --mask` rejected  (LOW)
`pentool image add ... --mask u` → `unexpected argument '--mask' found`. `docs/image-workflow.md` lists `--mask SHAPE_ID` / `--mask-fill-rule` as an add option; the flags exist only on `image set`. Accept on `add` or fix the docs.

## B04 — PDF text not extractable on v6 documents  (MED)
`repro_all.sh`: B04a passes (v5), **B04b fails (v6)**.
Same text + image document, v5 vs v6: `pdftotext` finds `HELLO` in the v5 PDF and nothing in the v6 PDF; both list one embedded font and the file sizes are within 50 bytes. 0.7.2 on v5 behaves like the good case, so this is a v6 regression (likely glyphs emitted without a ToUnicode/text layer). The page looks correct visually; text is just not selectable/searchable. Also: an image-heavy poster PDF was ~11 MB.

## B05 — Adjustments scale with canvas area  (HIGH, performance)
`repro_all.sh`: B05 (timing-based; flaky near the threshold, but the gap is ~50x).
One `hue-saturation` adjustment with `--scope ids:<one 375x305 image>`:

| canvas | export, no adj | export, 1 adj | cost |
|---|---|---|---|
| 400x320 | 78 ms | 91 ms | 13 ms |
| 1600x1340 | 228 ms | 471 ms | 243 ms |
| 3200x2680 | 784 ms | 1748 ms | 964 ms |

Cost grows ~4x per 4x canvas area while the target never changes size, so the adjustment is applied to a canvas-sized buffer even though the scope is one node. 13 images + 13 adjustments on a 1600x1340 canvas took ~5.5 s. This will make animation/video (many frames) unusable. **Expected:** cost proportional to the affected node's bounds. Also consider caching per-node results by content hash.

## B06 — Effects vanish on masked images  (HIGH)
`repro_all.sh`: B06.
```sh
# image v, vector circle mm -> mask create/attach -> hide mm (opacity 0)
pentool effect add e.pen v gl --kind outer-glow --color "#00FFFF" --blur 24 --opacity 1
pentool export e.pen e.png    # no glow anywhere; identical to export without the effect
```
The same glow on an **unmasked** image renders correctly. `drop-shadow`/`stroke` were fine on unmasked images; I did not test them on masked ones. **Expected:** effects computed from the masked silhouette (Photoshop layer-style behaviour), or at minimum rendered and clipped. Likely the effect pass reads the unmasked node alpha and is then clipped by the mask, leaving nothing outside the circle.

## B07 — `preset paste` drops the target's mask  (MED)
`repro_all.sh`: B07.
`preset copy` on image `u` (glow effect), `preset paste` onto masked image `v` → `v` keeps the glow but `mask` is gone, so the circular crop becomes a full rectangle. `clip add/remove` do **not** do this (checked). If masks are part of "appearance" it must be documented and opt-out; otherwise paste should merge effects/adjustments and leave the mask.

## B08 — Gradient fills: defaults and geometry  (MED)
`repro_all.sh`: B08, B08b.
- `fill add --kind linear --params '{"stops":[...]}'` (and radial) renders a **flat last-stop colour**; left and right pixels are identical. A gradient with no geometry should span the node's bounds.
- Geometry is shape-local pixels (`x1,y1,x2,y2`, `cx,cy,radius`). Passing page coordinates (natural, since `--x/--y` are page coordinates) also gives a flat colour with **no error or warning** (B08b: coordinates far outside the node accepted, exit 0).
- Working example for reference: node 480x140 at (40,40), `{"x1":0,"y1":0,"x2":480,"y2":0,"stops":[...]}` renders correctly.
- Conic/radial defaults for centre/radius also missing.

## B09 — `linked report` dumps base64  (LOW)
For ONE embedded 600x400 JPEG: **14 KB** of output (78 KB for two larger photos). Each dependency includes `storage.data`. Report digest, status, length and referencing node IDs only.

## B10 — `gradient-overlay` effect has no CLI-usable params  (LOW)
`pentool effect add f.pen c ov --kind gradient-overlay --opacity 0.4 --blend soft-light` → `Error: fill kind missing`. The expected `--params` shape isn't documented and there's no default. Same for `color-overlay`'s unclear defaults. Provide defaults or a clearer error naming the required key.

## B11 — Misleading error on `preset save`  (LOW)
`preset save FILE ID --file out.penpreset` works. Adding `--name X` (which selects the mutating "named appearance" variant) fails with `preset save does not mutate the document`. Error should say that `--name` is only valid with `copy/paste`.

## B12 — Can't list a node's effects  (LOW)
`pentool effect list FILE ID` → `required arguments were not provided: <EFFECT_ID>`. `adjustment list` and `mask list` work without extra IDs; `effect list` should too.

## B13 — Params undiscoverable  (MED, docs)
No docs or `--help` list the `--params` keys. Found by trial: `exposure {stops}`, `vibrance {amount}`, `color-balance {red,green,blue}`, `channel-mixer {matrix: 3 rows of R,G,B,offset}`, `gradient-map stops use "rgb":[r,g,b]` but `fill` stops use `"color":"#hex"`, `effect stroke {size}`, `selection --query` is JSON with kinds like `rectangle`/`ellipse`/`color-range`/`node-alpha`. `color-balance` reports only the alphabetically-first unknown key, which made every key look wrong. Please add a table to the docs and make the "unknown parameter" error list the valid keys.

## B14 — v6 documents export slower than v5  (MED, performance)
13 images, no adjustments, 1600x1340: **v5 430 ms, v6 1423 ms** (~3x). Each hue/sat adjustment then adds ~330 ms (see B05). Profile the v6 compositing engine when there is nothing to composite beyond plain images.

---

## Suspected root causes (to guide patching)
1. **Two render paths.** Documents with images take a different render-preparation route than plain ones. B01, B04b, B06 (and earlier 0.7.0/0.7.1 bugs) all look like "something the plain path does is skipped here". Unifying them would fix a whole class.
2. **Canvas-sized scratch buffers** in the adjustment/effect compositor (B05, B14).
3. **Appearance model doesn't distinguish effects from mask/clip** (B06, B07).
4. **Parameter validation is strict but undocumented** (B08b, B10, B13): either document it or make it self-describing.

## Already fixed (keep these as regression tests)
- `layer add`, `page list/add`, `text put` after `new` + `canvas` ("document has no canvas") — fixed 0.7.1.
- `tree` nests group children — fixed 0.7.1.
- Missing-file error is `[missing-resource]` with the path — fixed 0.7.1.
- Text vanishing from PNG/PDF when an image is present — fixed 0.7.1/0.7.2.
- Bold weights collapsing to regular with an image; italic whole-line fallback for a missing glyph (→) — fixed 0.7.2.
- Byte-identical exports; undo of adjustments byte-identical; checksum/`--if-revision`/`--dry-run` behaviour — all good in 0.8.0.

## Not bugs (to save time)
- `export` to `.jpg`/`.webp` is unsupported (PNG, SVG, PDF only); a feature gap, not a defect.
- `--crop` takes four space-separated values.
- History is stored beside the document (`.pentool/` next to the file), independent of the working directory. Note that deleting a shared `.pentool` folder erases history for every document in that folder.