# Pentool open asset and package protocol, version 1

This document defines the interoperable data and distribution protocol introduced
by Pentool v0.5 and v0.6. It is intentionally independent of any hosted Pentool
service. A conforming implementation may use local folders, Git, static HTTP,
object storage, mirrors, or a searchable hosted catalog.

## Principles

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
