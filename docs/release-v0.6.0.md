# Pentool v0.6.0 — open packages and reproducible libraries

Pentool v0.6 packages v0.5 assets into deterministic, data-only `.penpkg` archives
that can be published to replaceable filesystem or static HTTP registries.

Highlights:

- deterministic ZIP archives with canonical slash paths and SHA-256 asset/package hashes;
- archive path, duplicate-entry, compressed-size, expanded-size, download-size,
  redirect, timeout, and immutable-version checks;
- static registry search, publication, exact installation, and automatic activation
  in the project asset index;
- HTTPS registry reads with a verified content-addressed cache;
- cross-platform `pentool.lock`, verification, and offline restoration;
- optional Ed25519 publisher keys and detached package signatures;
- explicit instance update plans, conflict reporting, transactional replacement,
  recovery snapshots, and rollback;
- a host-independent protocol suitable for Git, static sites, object storage,
  self-hosted catalogs, mirrors, and future hosted discovery.

```sh
pentool package pack ./open-ui --output open-ui-1.0.0.penpkg
pentool package verify open-ui-1.0.0.penpkg
pentool package publish open-ui-1.0.0.penpkg --registry ./registry
pentool package install open-design/ui@1.0.0 --registry ./registry
pentool lock sync --offline
```

No registry or network connection is required to open, render, export, copy, or
detach an instance because its materialized fallback remains in the `.pen` document.
