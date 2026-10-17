# `.pen` format version 12: native vector gradient paints

Version 12 extends the version 11 ordered scene graph with native linear and
radial gradients in vector shape `style.fill.fallback` and
`style.stroke.fallback`. Existing solid color fallbacks and all prior node
properties remain valid. A gradient has 2–64 strictly increasing stops from
position 0 through 1, uses `srgb` or `linear` interpolation, and supports
`pad`, `repeat`, or `reflect` spread. Optional six-number transforms must be
finite, bounded, and invertible. Linear gradients carry `x1`, `y1`, `x2`, and
`y2`; radial gradients carry `cx`, `cy`, and a positive `radius`.

The importer currently supports SVG object-bounding-box linear and radial
gradients with solid color stops. SVG gradients using `userSpaceOnUse`, linked
definitions, focal points, or CSS URL paints are reported as approximated or
dropped according to the specific fallback. Gradients on editable text are
not represented. The CPU renderer and SVG exporter preserve supported shape
gradients.

The JSON schema documents the additive v12 structure. Rust validation remains
authoritative for cross-node checks, stop ordering, finite values, and matrix
invertibility.