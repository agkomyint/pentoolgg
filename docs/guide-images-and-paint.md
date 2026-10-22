# Images, photography, and painting

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [Optional image models](#optional-image-models)
- [RAW photographs](#raw-photographs)
- [Raster paint](#raster-paint)
- [Raster images](#raster-images)
- [Painting and image operations](#painting-and-image-operations)

## Optional image models

`pentool ai` connects your own Gemini or OpenAI-compatible account, generates reviewable
candidates, and accepts one into the document as an ordinary image asset. Setup is
non-interactive (`pentool ai setup --from-env`), keys are never stored, every model call
needs `--allow-model-call`, and nothing runs unless you configure it. See
[docs/ai.md](../docs/ai.md).

```sh
pentool ai setup --from-env && pentool ai doctor --check
pentool ai generate poster.pen hero --prompt "a fox" --allow-model-call
pentool ai run accept poster.pen RUN --candidate 1 --id hero
```

## RAW photographs

`pentool raw add` imports a DNG into the `.pen` v7 photography catalog and records
explicit import defaults. Other RAW formats are refused with advice to convert to
DNG. `pentool photo profile add` verifies and stores a DNG camera profile (`.dcp`),
and `pentool raw develop` sets a variant's camera profile, white balance, lens
correction, geometry, crop, upright and the development stack: tone (with auto
tone), presence and dehaze, curves, HSL, color grading, monochrome, vignette,
grain and calibration, plus detail: defective pixels, noise reduction, moiré,
defringe and capture sharpening, plus local adjustments (gradient, radial, range
and painted masks; `pentool photo mask paint`). `pentool photo render` develops a
raw variant into a tagged PNG (ICC and `cICP`), in any of the named RGB spaces
with perceptual or relative-colorimetric gamut mapping, or as Rec. 2020 PQ/HLG
HDR with measured `cLLI`; `pentool photo inspect` reports the histogram, gamut
and luminance without writing. `pentool photo merge-hdr` and `merge-pano` merge
brackets and panoramas into a new photo with a derived scene-linear DNG source.
`pentool photo variant` and `photo snapshot` keep virtual copies and saved settings
without duplicating sources, and `pentool photo settings sync` copies selected
settings groups to other photos in one transaction.
`pentool photo rate`, `photo keyword`, `photo stack`, `photo collection` and
`photo search` organize the catalog inside the document, and `pentool photo
contact-sheet` and `photo compare` write captioned PNG or PDF proof sheets.
Exports strip metadata by default: `pentool photo render --metadata
copyright|public|all-including-private` writes a deliberate EXIF/XMP subset, `photo
describe` sets title, caption, creator, copyright and location, `photo metadata`
shows a photo's metadata with GPS, serials and identity redacted, and `photo
privacy-report` lists sources that carry them.
`pentool photo export` plans, renders and stages a batch through a recipe
(`web-gallery`, `social`, `archive-master`, `photo-lab` or one stored with `photo
recipe set`) into JPEG, PNG or TIFF, and moves the files into place only when every
one succeeded; see [docs/photography-v1.md](../docs/photography-v1.md).
`pentool serve catalog.pen` opens the same catalog in a **Photos** studio. It has
a grid, loupe and filmstrip, keyboard culling, a histogram with clipping
warnings, clipping, gamut and mask overlays on a Display P3 or sRGB preview,
development sliders, crop, radial, linear and brush masks, variant compare, and
copy/sync. Every change is one undoable, revision-guarded edit made by the same
Rust engines. `pentool photo place` puts a developed variant on a page as a
`photo` node. Studio previews, `pentool photo preview` and the page renditions of
photo nodes are kept in a disposable cache in `.pentool/cache/photo/` (`photo
cache clear`, `--no-cache`), and `pentool
benchmark --photo` measures ingest, culling, development, previews and export on
a synthetic shoot.

```sh
pentool raw add catalog.pen hero --file ./capture.dng --external --dry-run
pentool raw info catalog.pen hero
pentool photo profile add catalog.pen --file ./studio.dcp
pentool photo profile add catalog.pen --lens ./lens.json
pentool photo profile import-lcp catalog.pen ./lens.lcp
pentool raw develop catalog.pen hero --temperature 5400 --tint 6
pentool raw develop catalog.pen hero --sample 0.42,0.31,0.01
pentool raw develop catalog.pen hero --suggest --dry-run
pentool raw develop catalog.pen hero --set lens.distortion=12 --set crop.constrain=true
pentool raw develop catalog.pen hero --upright guided --guide 0.21,0.1,0.23,0.9
pentool raw develop catalog.pen hero --auto-tone --set presence.dehaze=25
pentool raw develop catalog.pen hero --exposure -0.3 --set 'curves.point.rgb=[[0,0],[0.4,0.45],[1,1]]'
pentool raw develop catalog.pen hero --set 'detail.noise={"luminance":30,"color":25}' --set detail.sharpening.amount=60
pentool raw develop catalog.pen hero --set 'local=[{"id":"sky","mask":{"components":[{"kind":"linear","mode":"add","start":[0.5,0],"end":[0.5,0.45]}]},"params":{"exposure":-0.4}}]'
pentool photo mask paint catalog.pen hero --adjustment sky --samples '[[10,5,1],[30,5,1]]' --brush '{"size":8}'
pentool photo info catalog.pen hero
pentool photo render catalog.pen hero --out ./hero.png --space display-p3 --depth 16
pentool photo render catalog.pen hero --out ./hero-hdr.png --hdr pq --headroom 2
pentool photo inspect catalog.pen hero --space rec2020 --bins 32
pentool photo merge-hdr catalog.pen bracket-1 bracket-2 bracket-3 --id hero-hdr --deghost medium
pentool photo merge-pano catalog.pen pano-1 pano-2 pano-3 --id harbor --projection cylindrical --external merged/harbor.dng
pentool photo variant add catalog.pen hero warm-editorial --name "Warm editorial"
pentool photo snapshot add catalog.pen hero/warm-editorial before-grade
pentool photo snapshot restore catalog.pen hero before-grade
pentool photo settings sync catalog.pen hero --to harbor,pano-1/master --except crop,geometry --auto-per-photo
pentool photo rate catalog.pen hero,harbor --rating 4 --label green
pentool photo search catalog.pen "rating>=4 keyword:\"blue hour\"" --limit 20
pentool photo contact-sheet catalog.pen --selection "rating>=4" --columns 4 --out picks.pdf
pentool photo describe catalog.pen hero --creator "Ana Photo" --copyright "(c) 2026 Ana Photo"
pentool photo render catalog.pen hero --out hero.png --metadata public
pentool photo privacy-report catalog.pen
pentool photo export catalog.pen --selection "rating>=4" --recipe web-gallery --out ./delivery --dry-run
pentool photo export catalog.pen --selection hero --recipe photo-lab --print 6x4in --fit crop --out ./lab
pentool photo preview catalog.pen hero --out ./hero-preview.png --space display-p3 --overlay clipping
pentool photo place catalog.pen hero --layer content --x 40 --y 40 --width 600 --fit cover
pentool photo cache clear catalog.pen
pentool benchmark --photo --photos 24 --megapixels 2 --repetitions 3
```

## Raster paint

![Dusk landscape painted with raster layers](../examples/raster-paint-showcase.png)

Deterministic, replayable brush painting lives in `.pen` v6 `raster` layers. The
[showcase](../examples/raster-paint-showcase.md) paints this scene from a script
(`bash examples/raster-paint-showcase.sh`); see [docs/raster-paint-v1.md](../docs/raster-paint-v1.md)
for the full tool, selection, and batch reference.

## Raster images

In the served editor, use **Images → Add image** to choose a local PNG, JPEG or
WebP, select its page/layer, and give it a stable ID. Browser imports embed up to
32 MiB locally and are undoable. Older documents require the explicit v5-upgrade
checkbox; existing v6 documents keep their compositing format.
This also works with bare `pentool serve`: add the image to the browser document,
then use **Save .pen** to download it. Local Undo/Redo includes the import.

For compositing, explicitly migrate an older working copy
with `pentool migrate design.pen --target 6`, then add a non-destructive grade:
`pentool adjustment add design.pen grade --kind invert --scope below`.
Adjustments support `--params` JSON, opacity, enable/disable, dry runs, revision
guards and undo/redo. See the [development compositing contract](../docs/roadmap/v0.8.0/COMPOSITING-SPEC.md)
for all 13 adjustments, masks, clipping, all 16 blend modes, transforms, fills,
effects, selection/analysis queries, linked assets, appearance presets, batch
operations and editor controls. A tiny [v6 conformance fixture](../docs/fixtures/v6-composite.pen)
has exact analytic pixel tests.

```sh
pentool analyze design.pen --scope page --compare
pentool linked report design.pen
pentool linked collect design.pen --path portable-project --dry-run
pentool preset save design.pen portrait --file portrait.penpreset
pentool preset apply design.pen portrait-copy --file portrait.penpreset --dry-run
```

PNG, JPEG, and WebP sources can be embedded for portability or kept as verified,
document-relative external files. Imports are content-addressed, deduplicated, and
decoded offline with bounded dimensions and memory. Crop, fit, focal position,
opacity, transforms, and parent-space vector masks remain non-destructive.

```sh
pentool image add design.pen hero --file assets/hero.jpg --layer content --width 800 --height 450 --fit cover --external
pentool image set design.pen hero --crop 0.1 0 0.8 1 --position 0.5 0.4 --opacity 0.9
pentool image set design.pen hero --mask card-shape --mask-fill-rule evenodd
pentool image info design.pen hero
pentool export design.pen design.png --scale 2
pentool export design.pen design.svg
pentool export design.pen design.pdf
```

External sources must stay below the document directory and match their recorded
SHA-256 digest. Export fails before replacing an existing output when a source is
missing, changed, unsafe, malformed, or over its resource limit. Portable SVG
embeds normalized pixels; PNG and PDF use the same verified image pipeline.

## Painting and image operations

### Deterministic semantic raster strokes

Compact absolute SVG paths compile to the same canonical raster samples and
journal operations used by pointer strokes. Optional normalized curves vary brush
properties deterministically along path distance:

```sh
pentool raster portrait.pen stroke-path paint \
  --path 'M20 180 C80 20 220 20 300 180' \
  --brush '{"size":18,"spacing":0.2}' \
  --curves '{"size":[[0,8],[0.5,24],[1,10]],"pressure":[[0,0.2],[0.5,1],[1,0.4]]}' \
  --color '#8B5A3C' --seed 42
```

Line, quadratic, cubic, and arc segments are supported. Dry run, revision guards,
locks, bounded compilation, history, replay, batch, and the shared raster API use
the same planner. See [the raster-paint contract](../docs/raster-paint-v1.md).

### Color sampling and portable palettes

Sample through the authoritative renderer, extract deterministic palettes, mix
colors and generate seeded variations. Named palettes are readable document data
under `paint_palettes` with scope, color space, algorithm version and source hash:

```sh
pentool paint-palette portrait.pen sample --scope node:paint --x 40 --y 12
pentool paint-palette portrait.pen extract --scope node:paint --role shadow   --count 4 --save skin-shadows
pentool paint-palette portrait.pen mix '#8B5A3C' '#f1d3b8' --amount 0.25
pentool paint-palette portrait.pen vary '#8B5A3C' --seed 7 --amount 10 --count 6
pentool paint-palette portrait.pen list
```

Roles are `dominant`, `median`, `highlight`, `midtone`, `shadow` and `edge`.
Read-only actions reject `--dry-run`; saving and removing use the shared
transaction. See the "Color sampling and palettes" section of
[the protocol](../docs/agent-painting-protocol-v1.md).

### Region previews and structural comparison

```sh
pentool paint-review portrait.pen preview --scope node:paint --rect 0,0,128,128 --max-edge 256 --out look.png
pentool paint-review portrait.pen compare --scope node:paint --reference-image study.png --heatmap diff.png
pentool paint-review portrait.pen compare --scope page --reference-document earlier.pen
```

Metrics (`color_mae`, `value_mae`, `edge_disagreement`, `silhouette_iou`,
`structure_mae`, `texture_frequency_mae`) are exact integer ratios plus a six-place
decimal, with `changed_bounds`; they are guidance, not a quality score. Both
actions are read-only. See [the protocol](../docs/agent-painting-protocol-v1.md).

### Region-aware painting constraints

```sh
pentool raster portrait.pen select-marquee paint --rect 0 0 32 64
pentool raster portrait.pen select-save paint left
pentool raster portrait.pen stroke-path paint --path "M4 32 L60 32" --color "#ff0000"   --constraints '{"inside":["saved:left"],"follow":"contour"}'
pentool paint-constraints portrait.pen preview paint --spec '{"opaque_clip":true,"follow":"gradient"}' --out mask.png
```

Constraints (`inside`, `avoid`, `opaque_clip`, `barriers`, `reach_from`, `follow`)
resolve to one hashed, previewable mask; see [the protocol](../docs/agent-painting-protocol-v1.md).

### Stroke fields

```sh
pentool paint-fields grass.pen preview paint --field '{"method":"tensor","cell":4}'   --params '{"spacing":6,"length":30,"collision":"stop"}'
pentool raster grass.pen stroke-field paint --field '{"method":"radial","center":[128,128],"cell":8}'   --params '{"spacing":10,"length":60,"taper":0.3}' --brush '{"size":3}' --color '#2f6b2f' --seed 7
```

Fields come from contours, gradients, distance transforms, structure tensors,
guides or supplied vectors; strokes are seeded, collision-aware and pin the field
hash in the journal. See [the protocol](../docs/agent-painting-protocol-v1.md).

### Material paint modes

```sh
pentool paint-media list
pentool paint-media describe '{"kind":"impasto","light":45,"scale":4}'
pentool raster art.pen stroke-path paint --path "M4 32 L60 32" --color "#c05030"   --brush '{"size":14,"media":{"kind":"watercolor-wash","wetness":0.8,"wet_length":120}}'
```

Ten documented recipes (glaze, dry-brush, bristle, impasto, palette-knife,
watercolor-wash, charcoal, pencil, canvas-grain, paper-grain) with light
direction, tonal zones, preserve-luminance/chroma, pickup/deposit and wetness
lifetime. Procedural grain is canvas-fixed and integer-only, so strokes replay
exactly. See [the protocol](../docs/agent-painting-protocol-v1.md).

### Diagnostics and scoring

```sh
pentool paint-score before.pen after.pen --layer paint --goal '{"coverage":[0.2,0.5],"palette":["#204080"]}'
```

Plan passes report measured changed bounds, coverage, value range, palette drift,
edge spill and protected-region violations, plus warnings labelled measured or
heuristic. `paint-score` compares two revisions against a goal and never accepts or
reverts anything.

### Composition guides

```sh
pentool paint-guide portrait.pen add thirds --kind thirds --params '{"rect":[0,0,1200,1600]}'
pentool paint-guide portrait.pen snap thirds --point 410,805 --radius 20
```

Thirds, golden ratio, symmetry, horizon, perspective, vanishing point, gesture,
proportion, value-map and custom guides are stored as document data, never export
unless you ask for an SVG proof, and never change pixels. Stroke fields can use a
guide with `{"method":"guide","guide":"ID"}`.

### Paint plans

```sh
pentool paint-plan portrait.pen plan.json --dry-run
pentool paint-plan portrait.pen plan.json --if-revision REV --expect-plan-hash sha256:...
```

Runs an ordered list of stroke/field/fill passes as one atomic transaction with
per-pass changed bounds, dab and tile counts, enforced limits and one undo entry.
See [the protocol](../docs/agent-painting-protocol-v1.md).

### Paint bundles, templates and diff

```sh
pentool paint-bundle src.pen export --out look.json --brushes round --plans wash
pentool paint-bundle dst.pen --dry-run import look.json
pentool paint-bundle dst.pen plan-add wash plan.json
pentool paint-bundle dst.pen plan-show wash --out wash-plan.json
pentool paint-diff before.pen after.pen
pentool paint-diff --plans old-plan.json new-plan.json
```

A bundle carries brushes, palettes, guides and plan templates as data only. Import
validates every entry, never edits scene nodes (so component instances, masks and
overrides are untouched), and commits through the shared transaction (undo,
`--dry-run`, `--if-revision`). `paint-diff` summarizes added, removed and changed
paint data, per-pass and limit changes between plans, and changed tiles per layer.
Package manifests list each asset's paint data with hashes. See
[the protocol](../docs/agent-painting-protocol-v1.md).

### Visual analysis

```sh
pentool paint-analyze portrait.pen --scope node:paint --rect 300,200,400,400   --features contours,values,palette,orientation --values 5 --colors 8
```

Returns contour polylines, value zones, dominant colors, texture frequency,
gradient/stroke orientation, focal candidates, coverage gaps and clipping for a
bounded scope, tagged with renderer, revision and hashes, and paginated. It is
read-only. See [the protocol](../docs/agent-painting-protocol-v1.md).

### Non-destructive image operations

Image nodes carry an ordered operation stack evaluated top to bottom; the source
bytes never change until an explicit bake.

```sh
pentool image op add artwork.pen hero grayscale --op-id gray
pentool image op add artwork.pen hero blur --op-id soft --radius 2
pentool image op move artwork.pen hero soft --index 0
pentool image op disable artwork.pen hero gray
pentool image op list artwork.pen hero
pentool image analyze artwork.pen hero
pentool image bake artwork.pen hero --dry-run
pentool image bake artwork.pen hero --strip-metadata
pentool undo artwork.pen
```

Kinds: `crop`, `resize`, `rotate` (0/90/180/270), `brightness-contrast`, `levels`,
`curves` (`--points 0:0,128:160,255:255`), `hue-saturation`, `blur`, `sharpen`,
`grayscale`. A stack holds at most 64 operations; invalid parameters or stacks that
would exceed surface limits fail with `[invalid-operation]` or `[limit-exceeded]`
and leave the document unchanged. Use negative values as `--contrast=-20`.
Batch files may create a node with its whole stack atomically:

```json
[{"type":"put-image","id":"hero","layer":"content","data":"<base64>","width":160,"height":160,
  "operations":[{"kind":"grayscale","id":"g"},{"kind":"blur","id":"b","params":{"radius":1}}]}]
```

See [docs/params-reference.md](../docs/params-reference.md) for `--params` keys.

See [docs/image-workflow.md](../docs/image-workflow.md) for the complete raster image
workflow: storage, operations, bake, batch, packages, cache repair, the browser
panel, and benchmarks.
