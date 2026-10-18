# Pentool open asset and package protocol, version 1

This document defines the interoperable data and distribution protocol introduced
by Pentool v0.5 and v0.6. It is intentionally independent of any hosted Pentool
service. A conforming implementation may use local folders, Git, static HTTP,
object storage, mirrors, or a searchable hosted catalog.

## Principles

### Frozen agent-painting protocol

The specification-only v1 contract for deterministic semantic stroke requests,
data-only paint plans, stable results, content hashes, cancellation, hard resource
limits, and hostile-input handling is
[`agent-painting-protocol-v1.md`](agent-painting-protocol-v1.md). Its schemas and
fixtures freeze the interface before mutation commands are added. They do not by
themselves indicate that the v0.13 painting commands are implemented.

### Development v8 catalog compatibility

The developing [professional color contract](professional-color-v1.md) and
[v8 schema](pen-format-v8.schema.json) add a profile catalog through explicit
CLI migration. The browser accepts v8 scenes using the existing scene operations
and Rust validation. Existing revision guards and transaction history apply;
profile import alone does not change rendering. Native vector/background, fill,
effect and immutable vector mask paints validate in Rust and render through the
shared sRGB8 preview; their stored channels remain unchanged. Profile catalog and
paint assignment/conversion commands are currently CLI operations, with the same
revision guards and history. Persistent mask paint edits replace content identities
and their references atomically. Optional working/proof/display/output role
targets and intents are validated by Rust and consumed by CLI sample evaluation
and explicit paint conversion. Assignment pins ICC identities; profile removal
and downgrade refuse retained role dependencies. Read-only CLI native channel
interpolation uses the shared Rust evaluator with explicit space, intent and
separate alpha. Explicit v8 native fill gradients and gradient overlays now use
the same sampler in Rust before preview conversion; their space/intent can be
edited through existing fill operations. Legacy string-space gradients retain
their released interpolation. Interpolation destinations are profile dependencies
for removal, downgrade and package verification. Node and mask analysis resolve
their scoped native dependencies through the same preview planner.
Packaging checks the required role paths, including settings written directly
in JSON. Production compositing,
proof rendering and print editor controls remain under development.

### Development v6 editor API

The v6 development renderer and operation contract is specified in
[`roadmap/v0.8.0/COMPOSITING-SPEC.md`](roadmap/v0.8.0/COMPOSITING-SPEC.md).
`POST /api/scene` accepts composite batch operations with `page`, the semantic
`revision` returned by `GET /api/document`, and optional `dry_run`. The server
also guards the exact on-disk revision at commit, including against CLI writers.
One successful batch produces one document history entry.

`POST /api/composite/analyze` accepts `document`, `page`, `scope` (default `page`),
optional `query`, `samples` (pixel coordinate pairs) and `compare`. It returns
read-only `analysis` and optional `comparison`; no client color math is trusted.
`GET /api/composite/dependencies` reports actual linked-source status offline.

`GET /api/color/preflight` (`page`, `min_ppi`, `min_bleed_inches`) runs the print
preflight on the shared document and returns the report plus the resolved
`production` geometry (or `null`). It is read only and never changes the file.

`GET /api/color/plates` (`page`, `scale`) separates the shared document and lists each
plate's average and maximum coverage; adding `plate=ID` returns that plate as a
grayscale PNG (white is no ink). `GET /api/color/proof` (`page`) returns the
output-profile soft-proof image as a PNG; `GET /api/color/gamut` (`page`) returns the same page with pixels the output profile cannot reproduce shown in red. Both accept `intent` (`perceptual`, `relative-colorimetric`, `saturation`, `absolute-colorimetric`) as a proof-setup override of the output role's rendering intent; an unknown name answers 400. `GET /api/color/pdfx` (`page`, `ppi` 36..1200 default 300, `intent`, `compress`, `title`) downloads the flattened CMYK PDF/X-4 draft as `application/pdf`; it is read only and makes no conformance claim. All are bounded (about one million The proof and gamut views also accept `bpc=true` for linear XYZ black point compensation; the report carries `paper_simulation` (true under the absolute-colorimetric intent), `black_point_compensation` and `black_point_xyz_d50`.
pixels), read only, and answer 400 with `error` for unsupported pages or roles.

`POST /api/render/svg` and `/api/render/png` accept optional `page`, `max_edge`
(1..8192) and `job` query parameters. `max_edge` sizes the preview so the
selected page's longest edge is that many pixels: smaller than the canvas gives
a proxy, larger gives a sharper zoomed preview, up to 8x the canvas and about 50 million pixels
in total. Studio requests the size it displays. The request body must be the complete
document JSON from the `document` field returned by `GET /api/document`, not the
surrounding response object. It carries the `format` and `version` fields used to
select the renderer; an empty or partial document answers 400 with an `error`.
V6 proxy sizing bounds the output canvas,
not the persisted sources. SVG responses mark `x-pentool-preview` as approximate
when a proxy is requested. V6 has at most four active renderer workers.
`POST /api/composite/cancel` with `{ "job": "JOB_ID" }` signals cancellation;
workers stop at surface/operation boundaries and fill/warp scanlines. Existing
non-interruptible codec, font and blur work completes its current bounded stage.
Client abort and generation checks prevent stale results replacing newer previews.
Exports without `max_edge` evaluate authoritative full-resolution Rust output.

`POST /api/image/add` imports a local PNG/JPEG/WebP as an embedded image using
`id`, `page`, `layer`, base64 `data`, and the required semantic `revision`.
Browser imports are limited to 32 MiB of source bytes. The Rust image service
validates hashes, dimensions and codecs; the image is centered at its intrinsic
size or fitted within 80% of the selected canvas. Locked layers are rejected.
V1–v4 documents require explicit `migrate: true` to upgrade to v5; v6 remains v6.
Optional `dry_run` plans without writing. A successful import is one undoable,
revision-guarded transaction; malformed images and stale revisions do not mutate.
For a standalone browser document, `/api/image/add` and `/api/scene` accept an
explicit `document` and return the planned `document` without writing a shared
file, even if the server has one open. The editor records a local undo step and
requires Save .pen to persist it. Standalone imports still require explicit
format-migration consent; local plans never load linked files from the network.

### Photo studio API

`POST /api/photo/import?id=ID&name=FILENAME` accepts one DNG, JPEG or PNG as an
`application/octet-stream` body (up to 64 MiB) and requires the latest
`x-pentool-revision` header. The source is embedded in the shared `.pen` file
and committed as one undoable transaction. A stale revision or invalid image
leaves the file unchanged. JPEG/PNG photos support the shared rendered develop
controls; camera white balance, camera profiles, lens profiles and RAW controls
require DNG. The larger 512 MiB limit remains available through `pentool raw add`.

`POST /api/photo/place` accepts `photo`, `variant`, `page`, `layer`, `x`, `y`,
`width`, `height`, `fit`, `position`, and `revision`. It places the selected
developed variant on the requested page as one revision-guarded, undoable edit.
Missing photos/pages/layers, locked layers, or stale revisions leave the file
unchanged.

`GET /api/photo/catalog` (`query`, `offset`, `limit`) and `GET /api/photo/detail`
(`photo`) read the shared catalog and return its `revision`. `POST
/api/photo/preview` (`photo`, `variant`, `edge`, `space` `srgb`|`display-p3`,
`overlay` `none`|`clipping`|`gamut`|`mask:ID`, `uncropped`) returns `report` and a
base64 `png` tagged for the space; it never writes the document. Previews are kept
in `.pentool/cache/photo/` unless the server runs with `--no-cache`, and
`report.cache` is `hit`, `miss` or `off`. `POST /api/photo/edit`
(`edit`, `revision`, `dry_run`) applies one develop, rate, keyword, variant,
snapshot, restore, sync or paint edit through the CLI engines as one
revision-guarded, undoable transaction. A stale revision is `[conflict]` and any
failure leaves the file unchanged. The contract is "Photo editor" in
[`photography-v1.md`](photography-v1.md).

### Protocol principles

1. `.pen` files and `.penpkg` archives are portable data, never executable code.
2. Asset, package, and instance identity must not depend on absolute file paths.
3. Published package versions are immutable.
4. Content hashes identify exact bytes; semantic versions communicate intent.
5. Registries are replaceable discovery and transport services, not authorities
   required to render a document.
6. Installed content is verified before activation.
7. Documents containing instances remain renderable without a registry, package,
   network connection, or source library.
8. Updates are explicit, inspectable, transactional, and reversible.

## Identifiers

Portable identifiers are slash-separated ASCII segments. Each segment may contain
letters, digits, `.`, `_`, or `-`; empty, `.` and `..` segments are prohibited.

```text
package: open-design/icons
asset:   navigation/arrow-right
full:    open-design/icons/navigation/arrow-right
```

Package coordinates append an immutable semantic version:

```text
open-design/icons@1.4.0
```

Identifiers are case-sensitive. Publishers should use lowercase identifiers.
Registries must reject identifiers that collide after the case and Unicode
normalization rules of a supported target filesystem.

## Asset metadata

An asset is a valid `.pen` document with an optional top-level `asset` object:

```json
{
  "format": "pentool",
  "version": 3,
  "name": "Arrow right",
  "asset": {
    "schema": 1,
    "id": "navigation/arrow-right",
    "name": "Arrow right",
    "description": "Right-facing navigation arrow",
    "asset_version": "1.4.0",
    "kind": "component",
    "author": "Open Design",
    "license": "MIT",
    "tags": ["arrow", "navigation"],
    "category": "icons/navigation",
    "entry_page": "asset",
    "bounds": { "x": 0, "y": 0, "width": 24, "height": 24 },
    "properties": {}
  },
  "pages": []
}
```

`version` is the `.pen` format version. `asset_version` is the design asset's
semantic version. A `sha256:` content hash identifies an exact encoded asset.
Unknown metadata fields must be retained by editing and packaging tools.

## Local libraries

A local library is a registered directory scanned recursively for metadata-bearing
`.pen` assets. User libraries live in the platform-standard Pentool configuration
location. Project libraries live in `.pentool/libraries.json` and should use paths
relative to the project.

Indexes and thumbnails are disposable caches. They must be reconstructible from
source files and must not be committed as authoritative package state.

## Instances

A component instance records the source library/package, stable asset ID, semantic
version, exact content hash, imported layer IDs, transform, visibility, and approved
overrides. It also retains materialized `.pen` content as its fallback snapshot.
Open, render, export, copy, and detach must operate from this snapshot and must not
perform network access.

Source changes may mark an instance as outdated. They do not alter the document.
An update replaces the fallback only after verification and explicit acceptance.

## Package manifest

A `.penpkg` is a deterministic ZIP archive. It contains `pentool.package.json`,
metadata-bearing `.pen` files, optional previews, and optional license/readme data.
It cannot contain scripts, executable hooks, absolute paths, escaping paths, or
filesystem links.

```json
{
  "schema": 1,
  "name": "open-design/icons",
  "version": "1.4.0",
  "description": "Open interface icons",
  "license": "MIT",
  "repository": "https://example.org/open-design/icons",
  "authors": ["Open Design"],
  "pentool": ">=0.6.0",
  "assets": {
    "navigation/arrow-right": {
      "path": "assets/arrow-right.pen",
      "hash": "sha256:...",
      "preview": "previews/arrow-right.png"
    }
  },
  "dependencies": {
    "open-design/tokens": "^2.0.0"
  }
}
```

Archive entries are ordered lexicographically, use `/` separators, fixed metadata,
and deterministic compression settings. Manifest maps are ordered by key. The
package hash is SHA-256 over the complete archive bytes. Rebuilding identical
normalized input must produce identical bytes.

V8 asset entries can include an `icc_profiles` audit array. Each record contains
the original profile `digest`, `byte_length`, `icc_version`, `class`,
`color_space`, `connection_space`, declared `redistribution`, optional-source
value (`source`, null when absent), and sorted document-local `aliases`.
Every ICC catalog entry must be embedded and declare `allowed` redistribution.
Verification derives the array from the validated document and requires exact
agreement. Omitted arrays mean empty, preserving legacy manifests. `color profile
collect` prepares embedded dependencies; its private `--local-only` option does
not relax packaging's redistribution gate. Manifest-only and discovered assets
are both checked, and packaging archives their exact validated bytes.
Asset discovery skips private `.pentool` history/cache trees. Explicit file
references into those trees and preview paths colliding with assets, the manifest,
or the license are rejected. Included file paths must resolve within the package
root; discovery errors fail packaging instead of omitting an inaccessible asset.

## Registry protocol

The minimum registry is a static directory or HTTP origin:

```text
index.json
packages/<namespace>/<name>/<version>.penpkg
```

`index.json` schema 1 maps package names and versions to artifact paths, SHA-256
hashes, and yanked status. Paths are relative to the registry root. Static registries
need only immutable `GET` support; publishing may use Git, filesystem operations,
an object-store API, or a separate authenticated endpoint.

Clients must verify the downloaded archive hash and every asset hash before making
an installation active. Redirect, download, archive, expanded-size, file-count,
parse-time, and path limits are mandatory. A failing artifact is quarantined and
must not replace an installed version.

A hosted registry may add search, profiles, moderation, signatures, analytics, and
mirrors. Those services must not change package bytes or be required to interpret
the package format.

## Immutability, yanking, and deletion

The tuple `(registry, package name, version)` is immutable. Re-publishing identical
bytes is idempotent; different bytes are rejected. A version can be yanked to stop
new resolution while remaining downloadable for existing lockfiles. Permanent
deletion is reserved for legal or security emergencies and must leave a tombstone.

## Lockfile

`pentool.lock` records schema version and sorted exact resolutions:

```json
{
  "schema": 1,
  "packages": [{
    "name": "open-design/icons",
    "version": "1.4.0",
    "source": "https://registry.example/v1/",
    "package_hash": "sha256:..."
  }]
}
```

Locked and offline operations fail rather than choosing a different version.
Opening and exporting `.pen` documents never modify this file.

## Cache and installation

Packages are cached by verified content hash. Implementations should separate
download, quarantine, verified, and active states and use atomic writes plus
inter-process locks. An installation becomes visible to library discovery only
after all checks succeed. Cache pruning must protect active lockfile references.

## Signatures and trust

Checksums are mandatory. Protocol version 1 uses an optional JSON sidecar containing
an Ed25519 signature over the ASCII package hash, the signer public key, algorithm,
and schema version. Registries may advertise signer keys and projects may require signatures
or allow-list registries/publishers. Credentials never appear in manifests,
packages, lockfiles, URLs, logs, or exported registry configuration.

## Compatibility

Readers reject unsupported mandatory schema versions. Unknown fields are retained.
Clients may ignore unknown optional metadata but may not silently discard it while
rewriting source documents. Compatibility failures are reported before install or
instance update.

## Conformance

A conforming implementation must demonstrate:

1. deterministic rebuilds of the same package;
2. rejection of traversal, duplicate entries, hash mismatch, and changed immutable
   releases;
3. installation and lock verification without network access from a populated cache;
4. identical rendering of an instance after its source package is unavailable;
5. explicit update review and rollback without changing unrelated instances.
