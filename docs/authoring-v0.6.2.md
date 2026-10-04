# Design authoring with Pentool v0.6.2

Pentool v0.6.2 introduces the ordered v4 scene format. Reading a legacy file
never changes it; migrate explicitly and retain the history revision printed by
the command:

```powershell
pentool migrate app.pen --target 4
pentool history app.pen
```

## Build a card

Create primitives and bounded text, then group them. Text `y` is a baseline by
default; choose `--anchor top`, `center`, or `bottom` when that is more useful.

```powershell
pentool shape card.pen rrect card-bg --layer content --x 80 --y 80 --width 360 --height 220 --radius 24 --fill "#111827"
pentool text-box card.pen title --layer content --content "Agent-ready design" --x 112 --y 112 --width 280 --height 80 --anchor top --overflow ellipsis
pentool group card.pen create card-1 --layer content --children card-bg,title
```

For large scenes, submit the operations together. `docs/fixtures/v4-batch.json`
shows shapes, text, styles, groups, and temporary aliases in one transaction:

```powershell
pentool batch card.pen docs/fixtures/v4-batch.json
```

Batch operations may use flat style conveniences. Pentool normalizes these to the
nested v4 `style` representation, so the following path is equivalent to supplying
`style.fill`, `style.stroke`, and `style.stroke_width` objects directly:

```json
{
  "type": "put-path",
  "id": "outline",
  "layer": "content",
  "d": "M 0 0 L 100 0 L 100 100 Z",
  "fill": "none",
  "stroke": "#111827",
  "stroke_ref": "color.outline",
  "stroke_width": 2
}
```

`fill_ref` and `stroke_ref` retain the literal value as a fallback. Text accepts
flat `fill`/`fill_ref` or the corresponding nested style. Do not mix a flat and a
nested value for the same property in one operation; the flat value is explicit
and takes precedence.

## Reusable local components

```powershell
pentool group card.pen promote card-1 --component ui.card
pentool group card.pen instantiate card-2 --component ui.card --layer content --dx 400
```

Instances retain source identity and a renamed offline fallback snapshot. Existing
v0.5 package instances continue to use their established update, override,
detach, and rollback commands.

## Recolor with tokens or direct values

```powershell
pentool style app.pen set color.accent --type color --value "#F59E0B"
pentool style app.pen usage color.accent
pentool replace app.pen --all-pages --fill "#22D3EE" --to "#F59E0B" --dry-run
```

Token references retain explicit fallbacks. Missing tokens use their fallback;
alias cycles are rejected.

## Lay out a grid

```powershell
pentool layout app.pen grid --ids card-1,card-2,card-3,card-4,card-5,card-6,card-7,card-8,card-9 --columns 3 --gap 24
pentool layout app.pen left --ids card-1,card-4,card-7 --dry-run
```

Dry-run output includes measured selection bounds and every applied delta.

## Undo and restore

History is content-addressed under `.pentool/history/`; normal edits do not create
sibling `.bak.N` files.

```powershell
pentool history app.pen
pentool undo app.pen
pentool redo app.pen
pentool restore app.pen --revision SHA256
pentool history prune app.pen --keep 50 --dry-run
```

## Export and compare

```powershell
pentool export deck.pen exports --all-pages --format png
pentool export deck.pen deck.pdf --all-pages
pentool diff old.pen new.pen --visual --json
pentool format deck.pen --compact --dry-run
```

Page filenames are stable and collision-safe through their numeric prefix.
Multi-page directory export is staged before replacement. PDF output keeps vector
paths and searchable built-in-font text. Use SVG outline export when exact custom
font glyph appearance matters.

## Browser authoring

`pentool serve app.pen` opens v4 documents without migration. The Scene operations
panel sends the same validated batch operations used by the CLI and provides
project-history undo/redo. The preview uses the shared Rust SVG renderer.
