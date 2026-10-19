# Pentool v0.21.1

Patch release. No format, CLI, or output changes.

## Performance

- Faster compositing of pages with many objects. Studio previews and PNG/PDF
  exports of a 50-object page at 2048 px render roughly three times faster.
  Output bytes are unchanged:
  - the rasterized node surface is converted to straight alpha in place, and
    transparent and opaque pixels, which convert to themselves, are skipped;
  - blending skips pixels where the source is fully transparent under separable
    sRGB modes, and full-opacity layers skip the opacity pass.
