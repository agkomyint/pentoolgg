# Pentool v0.5.0 — local libraries and reusable components

Pentool v0.5 turns normal `.pen` documents, pages, layers, objects, and rectangular
selections into reusable assets with stable IDs and semantic versions.

Highlights:

- project and user library registration, refresh, enable/disable, removal, and diagnostics;
- deterministic indexed search with filters and pagination;
- content-hash keyed PNG/SVG preview caching;
- copy placement through the v0.4 transactional composition engine;
- offline-safe component instances with materialized layers, exposed-property guards,
  inspection, and detach;
- a browser Asset Explorer that searches the same Rust index, places copies or
  instances, and exports an active layer as a reusable asset;
- isolated warnings for missing folders, invalid assets, duplicates, and scan errors.

```sh
pentool asset create design.pen assets/card.pen --id ui/card --name Card --layer card
pentool library add ./assets --name project-assets
pentool explore card
pentool add poster.pen project-assets/ui/card --mode instance --at 100 80
```

Asset and instance extensions are documented in `docs/pen-format.md` and the open
distribution contract is specified by `docs/PROTOCOL.md`.
