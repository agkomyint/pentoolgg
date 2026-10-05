# Raster images: complete workflow (v0.7)

Every command below is runnable against a v4/v5 `.pen` document. Mutating commands
accept `--dry-run` and `--if-revision`, commit through the shared transaction
engine, and create exactly one history entry. A failed command leaves the document
byte-for-byte unchanged. Nothing here contacts the network.

## Import and storage tradeoffs

| Mode | Flag | Portable | Document size | Notes |
| --- | --- | --- | --- | --- |
| Embedded | `--embed` (default) | yes | grows by base64 source | Single-file hand-off; packages carry the blob. |
| External | `--external` | only with the file | small | Path must stay inside the document directory; symlinks that escape are rejected. |

Sources are PNG, JPEG, or WebP, identified by `sha256:` digest and deduplicated:
placing the same bytes 100 times stores one blob. Hash, dimension, pixel, and
memory limits are checked before decoding. EXIF orientation is applied; EXIF, GPS,
and text chunks never survive derived or baked output.

```sh
pentool new art.pen
pentool image add art.pen hero --file photo.jpg --layer layer-1 --width 800 --height 450 --fit cover --embed
pentool image info art.pen hero
```

## Crop, fit, and mask semantics

`crop` is a normalized `[x, y, w, h]` rectangle of the source. `fit` (`fill`,
`contain`, `cover`, `none`, `scale-down`) maps the cropped region into the frame;
`position` (`[0..1, 0..1]`) is the focal alignment inside any slack. A mask is a
vector shape in the parent space (`--mask SHAPE_ID`, `--mask-fill-rule`). The frame
always clips the result.

## Operations

Operations form an ordered stack of at most 64 per image, applied top to bottom on
premultiplied alpha with deterministic libm-free arithmetic. Toggling, reordering,
and editing never touch the source bytes.

| Kind | Parameters |
| --- | --- |
| `crop` | `x y width height` (normalized) |
| `resize` | `width height` (pixels, bounded surface) |
| `rotate` | `degrees` = 0, 90, 180, 270 |
| `brightness-contrast` | `brightness contrast` (-100..100) |
| `levels` | `black white gamma` |
| `curves` | `points` as `in:out,…` (CLI) or `[[in,out],…]` |
| `hue-saturation` | `hue saturation` |
| `blur` | `radius` (0–256) |
| `sharpen` | `radius amount` (amount 0–500) |
| `grayscale` | — |

```sh
pentool image op add art.pen hero blur --op-id soft --radius 2
pentool image op add art.pen hero brightness-contrast --brightness 10 --contrast=-5
pentool image op move art.pen hero soft --index 0
pentool image op disable art.pen hero soft
pentool image op list art.pen hero
```

Errors carry bracketed codes: `[invalid-operation]`, `[limit-exceeded]`,
`[unsupported-capability]`, `[hash-mismatch]`, `[missing-resource]`,
`[unsafe-path]`, `[malformed-resource]`. Use `--json` for stable machine-readable
errors.

## Bake

`image bake` flattens the crop and enabled operations into a new embedded,
metadata-free PNG, records provenance, prunes sources nothing references, and is
fully undoable. `--dry-run` predicts the result without writing.

```sh
pentool image bake art.pen hero --dry-run
pentool image bake art.pen hero
pentool undo art.pen
```

## Analyze

`pentool image analyze art.pen hero` is read-only: dimensions, alpha share,
dominant colors, and a focal-point suggestion. To keep the palette, run the explicit
follow-up (one transaction, supports `--dry-run`, `--if-revision`, and `undo`):

```sh
pentool image palette art.pen hero --prefix brand --count 3
pentool style art.pen list
```

## Batch

```sh
cat > ops.json <<'JSON'
[
  {"type":"put-image","id":"badge","layer":"layer-1","data":"<base64>","width":160,"height":160,
   "operations":[{"kind":"grayscale"},{"kind":"blur","params":{"radius":1}}]},
  {"type":"image-op-add","id":"badge","op":"sharpen","params":{"radius":1,"amount":50}}
]
JSON
pentool batch art.pen ops.json
```

The whole batch validates before any write; one failing item aborts everything.

## Export, linked SVG, and portability

```sh
pentool export art.pen out.png --scale 2
pentool export art.pen out.svg                  # portable: pixels embedded
pentool export art.pen out.svg --link-images    # references verified external files
pentool export art.pen out.pdf
```

Linked mode fails closed: any node whose pixels differ from the source file
(crop, operations, orientation) is refused rather than shown incorrectly.

## Packages and cache repair

`package pack/verify/install` carry embedded image blobs and a deterministic
per-asset table (digest, media type, dimensions, length, consumers); `verify`
cross-checks it. External image files are rejected from packages—embed first.

External sources are verified by hash on every read. `pentool image cache art.pen`
copies verified external sources to `<document dir>/.pentool/cache/sha256/<hex>`,
so rendering still works offline after the original moves. If the original exists
but its bytes changed, rendering fails with `[hash-mismatch]`; the cache is never
used to hide a changed file. A corrupt cache entry is ignored and rewritten.
Large processed results are cached under `.pentool/cache/processed/`; the cache is
an accelerator only and deleting or corrupting it never changes output.

## Browser editor

`pentool serve art.pen` shows an **Image operations** panel: operation list with
enable, reorder, and remove; add by kind; reset; before/after; and bake with a
dry-run confirmation. Commits go through `/api/scene` and `/api/image/bake`.
Previews rendered with `?max_edge=N` use reduced pixel detail and carry
`x-pentool-preview: approximate`; exports are always authoritative.

## Performance

```sh
pentool benchmark --images 100 --source-size 512 --operations 2 --repetitions 5
```

Reports cold and warm runs, reuse count, operation count, document size, stage
times, and peak memory. See [performance.md](performance.md) for budgets.

## Color limitations and external tools

Pixels are treated as 8-bit sRGB RGBA; embedded ICC profiles are not applied and
are not preserved in derived output. RAW development, ICC conversion, and
non-PNG/JPEG/WebP codecs are out of scope for v0.7; use an external tool and
re-import the result.
