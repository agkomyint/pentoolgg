# `--params` reference (v0.8)

`adjustment add/set`, `effect add/set`, and `fill add` take `--params` as a JSON
object. An unknown key is rejected and the error lists the valid keys.

## Adjustments

| kind | keys |
|------|------|
| `exposure` | `stops` |
| `vibrance` | `amount` |
| `color-balance` | `red`, `green`, `blue` |
| `black-and-white` | `red`, `green`, `blue` |
| `channel-mixer` | `matrix`: 3 rows of `[R, G, B, offset]` |
| `gradient-map` | `stops`: `[{"offset":0,"rgb":[r,g,b]}, ...]` (note `rgb`, not `color`) |
| `posterize` | `levels` |
| `threshold` | `level` |
| `brightness-contrast`, `levels`, `curves`, `hue-saturation` | same keys as the matching image operation |

Adjustment cost is proportional to the scoped node's bounds, not the canvas.

## Effects

| kind | keys |
|------|------|
| `drop-shadow`, `inner-shadow` | `x`, `y`, `blur`, `color` |
| `outer-glow`, `inner-glow` | `blur`, `color` |
| `stroke` | `size`, `color`, `position` |
| `color-overlay` | `color` |
| `gradient-overlay` | `fill`: a fill object (`kind`, `stops`, ...); defaults to a node-wide linear gradient |

`pentool effect list FILE NODE_ID` lists a node's effects; add `EFFECT_ID` for one.
Layer-style order is mask, then effects (computed from the masked silhouette),
then opacity.

## Fills

`fill add --kind linear|radial|conic --params '{"stops":[{"offset":0,"color":"#fff"}, ...]}'`

Geometry is **node-local** pixels (`x1,y1,x2,y2`; `cx,cy,radius`). When omitted it
spans the node's bounds (radial/conic centre = node centre). Coordinates entirely
outside the node are rejected rather than rendering a flat colour. Fill stops use
`"color":"#hex"`.

## Selections

`--query` is JSON with `kind` one of `rectangle`, `ellipse`, `color-range`,
`node-alpha`.
