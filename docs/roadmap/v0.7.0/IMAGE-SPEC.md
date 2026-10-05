# Pentool raster image specification

Status: frozen for Pentool v0.7.0. The document format version is 5. The
normative machine contract is [`../../pen-format-v5.schema.json`](../../pen-format-v5.schema.json)
and the security analysis is [`THREAT-MODEL.md`](THREAT-MODEL.md).

This document defines deterministic raster-image placement and non-destructive
editing for `.pen` documents. It is normative where it uses MUST, MUST NOT, SHOULD,
and MAY. The corresponding delivery plan is in [`TASKS.md`](TASKS.md).

## 1. Goals

Pentool images MUST preserve the product's existing properties:

- readable, reviewable `.pen` documents;
- deterministic offline rendering;
- one self-contained Rust executable per supported target;
- explicit, content-addressed assets and reproducible packages;
- agent-friendly CLI operations with dry runs and structured output;
- the same results from the CLI, Rust library, server API, and browser editor.

v0.7 is image placement and adjustment, not a pixel-painting application. The
source image remains immutable. Edits are represented as an ordered operation
stack and are flattened only through an explicit bake operation.

## 2. Relationship to v0.6.2

v0.7 MUST build image nodes on the ordered scene graph, transactions, styles,
groups, layout, history, and migration contract introduced by v0.6.2. An image is
a normal scene node: it can be grouped, transformed, aligned, distributed, masked,
duplicated, diffed, placed in a component, and addressed by stable ID.

Version 5 extends the v4 ordered scene graph with image assets and image nodes.
Versions 1–4 remain readable. Migration to v5 is explicit; ordinary edits MUST NOT
silently upgrade an older document. Readers MUST reject unsupported newer versions
and unknown required node kinds with a clear error; they MUST NOT silently omit
them.

## 3. Content-addressed image assets

Every imported source is identified by the lowercase hexadecimal SHA-256 digest of
its exact original bytes. Documents keep asset records separately from image nodes,
so multiple nodes can reuse one source without duplicating it.

Conceptual representation:

```json
{
  "image_assets": {
    "sha256:0123...cdef": {
      "media_type": "image/png",
      "byte_length": 42831,
      "pixel_width": 1600,
      "pixel_height": 900,
      "color_space": "srgb8",
      "orientation": 1,
      "storage": {
        "kind": "embedded",
        "encoding": "base64",
        "data": "..."
      }
    }
  }
}
```

The stored digest MUST cover source bytes, not decoded pixels. Import MUST decode
and validate the source before commit. Asset metadata MUST match the decoded source.
Duplicate source bytes MUST reuse the same asset record.

Asset keys and `asset` references use the exact form `sha256:` followed by 64
lowercase hexadecimal digits. `media_type` is one of `image/png`, `image/jpeg`, or
`image/webp`. Dimensions describe the display-oriented image after applying EXIF
orientation; `orientation` records the parsed source value from 1 through 8.

### 3.1 Embedded storage

Embedded assets store base64 source bytes in the document. Implementations MUST
enforce per-asset and per-document encoded and decoded byte limits before
allocation. Default limits belong in one documented security policy and MAY be
lowered through configuration, never silently raised by document data.

The v5 defaults are 128 MiB source bytes per asset, 512 MiB embedded source bytes
per document, 32,768 pixels on either axis, 268,435,456 decoded pixels per asset,
and 1 GiB for any single decoded RGBA8 surface. Base64 length is checked before
decode. Checked arithmetic is mandatory for every derived byte count.

### 3.2 External storage

External assets contain a normalized document-relative path and mandatory digest.
Absolute paths, parent traversal, device paths, alternate data streams, symlinks
escaping the project root, and ambiguous case collisions MUST be rejected.

Opening a document MAY report an unavailable external source. Rendering MUST NOT
substitute different bytes: it either uses a verified local source, a verified
cache entry, or returns a structured missing-asset error. A hash mismatch is a
verification failure, not a cache miss.

### 3.3 Packages and cache

`.penpkg` archives store image blobs under content-addressed paths and list their
digest, media type, dimensions, byte length, and consumers in the manifest. Package
verification MUST apply the same decoder, pixel, memory, and hash policies used at
import. The v0.6 verified cache SHOULD store immutable source blobs by digest.

Opening, previewing, or rendering MUST NOT fetch a URL or resolve a package over the
network. Remote acquisition is an explicit install/import operation completed
before rendering.

## 4. Image node

An image node contains at least:

```json
{
  "kind": "image",
  "id": "hero-photo",
  "asset": "sha256:0123...cdef",
  "x": 80,
  "y": 120,
  "width": 640,
  "height": 360,
  "fit": "cover",
  "position": [0.5, 0.5],
  "crop": [0, 0, 1, 1],
  "opacity": 1,
  "blend_mode": "normal",
  "transform": [1, 0, 0, 1, 0, 0],
  "operations": []
}
```

- `asset` MUST resolve to a verified image-asset record.
- `width` and `height` MUST be finite and greater than zero.
- `fit` is `fill`, `contain`, `cover`, `none`, or `scale-down`.
- `position` is normalized focal position `[x, y]` in the inclusive range 0–1.
- `crop` is a normalized source rectangle `[x, y, width, height]` inside 0–1.
- `opacity` is finite and in the inclusive range 0–1.
- v0.7 REQUIRED blend mode is `normal`. Additional modes require specified color
  math, conformance fixtures, and cross-platform golden tests before becoming
  portable document features.
- `transform` follows the scene graph's local affine-transform contract.
- an optional `mask` references a vector node or inline immutable mask snapshot;
  v5 supports a reference object `{ "node": "mask-id", "space": "parent",
  "fill_rule": "nonzero" }`. The referenced path, primitive, or group is evaluated
  in the image node's parent coordinate space. `fill_rule` is `nonzero` or
  `evenodd`. Missing references, references to non-vector nodes, cross-page
  references, self-reference, and dependency cycles are validation errors. Inline
  mask snapshots are reserved for a later format version.

Bounds commands MUST distinguish frame bounds, cropped source bounds, transformed
world bounds, and visible mask-intersected bounds.

## 5. Decode and color contract

The default build MUST decode PNG, JPEG, and WebP using audited pure-Rust crates.
Format detection MUST use validated content signatures rather than filename alone.
Malformed, truncated, oversized, animated, or unsupported inputs return structured
errors and MUST NOT partially modify a document or cache.

For v0.7, imports MUST decode into sRGB with 8-bit RGBA working pixels. Embedded
ICC profiles MAY be used for conversion if the implementation is deterministic;
otherwise the importer MUST report that it normalized or ignored unsupported color
metadata. The stored source remains byte-exact, but the normalized color-space
decision is recorded in asset metadata.

Internal filtering and compositing MUST use premultiplied alpha. Operations that
conceptually work on color channels MUST specify when unpremultiplication and
clamping occur. Transparent RGB values MUST NOT leak visible fringes after resize,
blur, masking, or compositing.

EXIF orientation MUST be applied deterministically on import or represented as an
explicit initial transform. EXIF, GPS, thumbnails, and other private metadata MUST
be stripped from baked/exported assets by default. `--keep-metadata` is explicit,
reports retained classes, and MUST never preserve unknown metadata silently.

Animated PNG/WebP and multi-image sources are out of scope for v0.7 unless an
explicit frame is selected during import. There is no implicit animation.

## 6. Non-destructive operation stack

Operations are evaluated in array order. Each operation has a stable ID, kind,
version, enabled flag, and validated parameters. Unknown required operations make
the document unsupported; they MUST NOT be skipped silently.

Required v0.7 operations:

- crop;
- resize;
- rotate;
- brightness and contrast;
- levels;
- curves;
- hue and saturation;
- gaussian blur;
- sharpen;
- grayscale.

Each operation specification MUST define parameter units and ranges, edge handling,
sampling kernel, interpolation domain, rounding, clamping, alpha behavior, and
empty-image behavior. Implementations MUST pin processing math tightly enough that
golden pixels match on all release targets. Fixed-point math is preferred where it
does not materially reduce output quality; controlled `f32` requires explicit
tolerances and architecture tests.

Operations MUST be serializable, diffable, reorderable, individually disableable,
and addressable by stable ID. An edit changes parameters, not source bytes.

### 6.1 Bake

`image bake` evaluates crop, fit-independent pixel operations, and masks as selected
by explicit flags, encodes a new source blob, records its provenance, and repoints
the node only after verification. The previous source and operation stack remain
recoverable through document history. Bake MUST report source hash, result hash,
codec, dimensions, byte size, removed operations, and metadata policy in dry-run
and committed results.

## 7. Deterministic caching

Processed results and proxies are caches, never authoritative document state. A
cache key MUST include:

- source SHA-256;
- canonical operation-stack serialization;
- crop/mask content hashes where applicable;
- output pixel dimensions;
- working color-space and alpha contract version;
- decoder, processor, and resampling algorithm versions.

Cache hits MUST be byte/pixel equivalent to recomputation. Corrupt or incompatible
entries are ignored and safely replaced. Cache writes are atomic and use the v0.6
verified-cache locking and bounded-pruning patterns.

The editor SHOULD use content-addressed thumbnails and mip levels. Server-side Rust
rendering remains authoritative; browser previews MAY use proxies but must expose
when a preview is approximate and refresh from the authoritative result.

## 8. CLI and API contract

Minimum CLI:

```sh
pentool image add poster.pen hero --file ./hero.jpg --layer content \
  --x 80 --y 120 --width 640 --height 360 --fit cover --embed
pentool image set poster.pen hero --fit contain --opacity 0.9
pentool image info poster.pen hero --json
pentool image analyze poster.pen hero --json
pentool image op add poster.pen hero blur --radius 4
pentool image op list poster.pen hero --json
pentool image op set poster.pen hero <op-id> --radius 8 --dry-run
pentool image op move poster.pen hero <op-id> --index 0
pentool image op remove poster.pen hero <op-id>
pentool image bake poster.pen hero --format png --strip-metadata
```

All writes use the v0.6.2 transaction/history engine and support `--dry-run`,
revision guards, JSON results, page/group/layer context, and exactly one undo entry.
`tree`, `search`, `object`, `batch`, `diff`, groups, components, align/distribute,
packages, and export MUST understand image nodes.

Batch MUST support asset import references, image-node creation, image-property
updates, and operation add/set/remove/reorder. A failed batch commits neither blobs
nor document changes. Imports SHOULD support predeclared content hashes so callers
can detect source changes between planning and commit.

### 8.1 Analysis

`image analyze` returns decoded dimensions, aspect ratio, alpha presence, source
format, color-space handling, content hash, dominant colors, and an optional
suggested focal point with algorithm/version metadata and confidence. Analysis MUST
be deterministic, bounded, offline, and read-only. Suggested colors do not mutate
document tokens until an explicit style command applies them.

## 9. Rendering and export

SVG export SHOULD use a data URI or verified relative asset according to explicit
portability options. Portable/package export MUST embed or package the verified
bytes. PNG and PDF use the authoritative Rust decode/process/composite pipeline.

The renderer MUST apply source orientation, normalized crop, operation stack,
resampling, fit, mask, opacity, transform, and compositing in a specified order.
That order is part of the file-format contract and requires golden fixtures.

The v5 order is: verify source bytes; decode; apply source orientation; crop in
oriented source coordinates; evaluate enabled operations in array order; calculate
`fit` and `position` into the image frame; resample with the pinned engine version;
apply the parent-space vector mask; multiply premultiplied alpha by node opacity;
apply the node/world transform; composite with source-over normal blending.

External assets are hash-verified before decode. Render never rewrites source
assets, updates operation stacks, downloads bytes, or mutates caches required for
correctness. Cache failure may reduce performance but not change output.

## 10. Security and resource limits

Before decoding or allocating pixels, implementations MUST enforce checked limits
for source bytes, dimensions, total pixels, row bytes, decoded bytes, operation
expansion, intermediate surfaces, mask surfaces, and total render memory. Arithmetic
overflow is an error. Limits apply cumulatively to documents and packages as well
as individually to assets.

Required security work:

- fuzz every enabled decoder and image/operation schema parser;
- fuzz operation stacks, crop rectangles, masks, cache records, and package images;
- test decompression bombs, malformed chunks, extreme dimensions, hash mismatch,
  path traversal, symlink escapes, truncated data, and allocation failure;
- run dependency license/advisory/source policy in CI;
- record and review decoder upgrades separately from unrelated dependency updates;
- sign release archives and `SHA256SUMS.txt` through the existing release process.

No decoder, operation, external tool, or renderer receives network authority.

## 11. Single-binary distribution

Default artifacts MUST remain single executables with no required system image
libraries. The default feature set includes PNG, JPEG, and WebP. Optional codecs
such as AVIF, HEIC, or JPEG XL may ship in a separately named `pentool-full`
artifact only when they use audited dependencies compatible with this distribution
model and pass the same determinism, security, size, and cross-platform tests.

Codec features MUST NOT change rendering of already supported formats. CI records
stripped binary size by target and fails on an approved absolute or relative size
budget. `cargo bloat` or equivalent analysis SHOULD accompany size regressions;
size estimates are not accepted without measurement.

## 12. External image-tool protocol

AI background removal, inpainting, generative fill, and model inference are outside
the core binary. A future external-tool protocol MAY exchange a versioned JSON
request on stdin, local content-addressed input/output handles, progress/events on
stderr or a separate channel, and a JSON result on stdout.

Results imported from tools MUST store provenance: tool identifier and version,
input hashes, canonical parameters, output hash, declared model identifier, and
whether deterministic reproduction is expected. Tools are explicit user-invoked
processes, receive least authority, and are never executed merely by opening,
rendering, installing, or previewing a document/package.

## 13. Compatibility and conformance

The repository MUST include normative documents for embedded/external assets,
every operation, masks, alpha edges, EXIF orientation, metadata stripping, missing
assets, bad hashes, and resource-limit failures. Golden tests run on Windows,
Linux, Intel macOS, and Apple Silicon macOS.

Conformance covers both serialized output and decoded pixels. If exact encoded PNG
bytes cannot remain stable across a justified encoder upgrade, decoded pixel hashes
remain normative and encoded-byte changes require an explicit compatibility note.

The initial normative documents are
[`v5-image-embedded.pen`](../../fixtures/v5-image-embedded.pen),
[`v5-image-invalid-hash.pen`](../../fixtures/v5-image-invalid-hash.pen),
[`v5-image-invalid-path.pen`](../../fixtures/v5-image-invalid-path.pen), and
[`v5-image-invalid-node.pen`](../../fixtures/v5-image-invalid-node.pen). A fixture
whose name contains `invalid` MUST be rejected before mutation or render output.
