# v0.7 image threat model

This threat model is normative for Pentool v0.7 image import, open, render,
package, cache, and bake operations.

## Trust boundaries

`.pen`, `.penpkg`, PNG, JPEG, WebP, metadata, external paths, cache entries, and
operation parameters are untrusted. Opening or rendering a document grants no
network or process-execution authority. Source bytes become trusted only for the
specific digest after bounded signature validation, hash verification, and decode.

## Required defenses

- Reject source data over 128 MiB, dimensions over 32,768 on either axis, decoded
  images over 268,435,456 pixels, and any single RGBA8 surface over 1 GiB before
  allocation. Document embedded sources are cumulatively limited to 512 MiB.
- Use checked integer arithmetic for base64 size, rows, pixels, intermediate
  surfaces, masks, operation expansion, render totals, and package expansion.
- Detect formats from bytes and reject mismatched media types, truncation,
  unsupported animation/multiple frames, malformed metadata, and decoder errors.
- Verify external and cached bytes before decode. Never fall back to unverified
  bytes with the same filename or media type.
- Accept only normalized `/`-separated document-relative external paths. Reject
  absolute, parent, backslash, drive/device, UNC, alternate-stream, empty-segment,
  NUL, and symlink-escape paths on every host platform.
- Treat image and operation kinds as required capabilities. Unknown kinds or
  versions fail with `unsupported-capability`; they are never skipped.
- Validate mask targets and dependency cycles before render allocation.
- Strip EXIF, GPS, thumbnails, comments, and unknown metadata from derived/baked
  output unless a reviewed explicit retention option names each retained class.
- Stage source/cache writes with the document transaction. Failure leaves the
  document, resources, cache, and history unchanged.

## Stable error classes

Use `unsupported-capability`, `limit-exceeded`, `malformed-resource`,
`hash-mismatch`, `missing-resource`, and `unsafe-path`. Diagnostics add asset,
node, operation index/ID, page, and path context without including source bytes.

## Non-goals

The core does not fetch URLs, invoke codecs or AI tools as subprocesses, execute
metadata, preserve animation, or trust filename extensions. Cache availability
may affect performance but never pixels or correctness.
