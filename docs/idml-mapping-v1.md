# IDML ⇄ `.pen` mapping 1

Status: frozen mapping contract for the v0.14.0 roadmap (task 1). `pentool
idml-import` (`src/idml.rs`) and `pentool idml-export` (`src/idml_export.rs`)
implement the subset listed under [Implemented subset](#implemented-subset);
everything else in the table is still the contract, not behavior. Layout
semantics are in [`layout-v1.md`](layout-v1.md).

IDML is Adobe's published XML interchange format (a ZIP of XML parts). Pentool
reads and writes IDML only through that published format; it uses no Adobe code
or resources, and refuses native `.indd`/`.indb` binaries with: *"Pentool cannot
open native InDesign files. In InDesign choose File › Export › InDesign Markup
(IDML) and open that file."*

## Status vocabulary

| Status | Meaning |
| --- | --- |
| **supported** | Imported into a native construct and exported back with the same value. Round trip is exact for supported values. |
| **approximated** | Imported into the nearest native construct; the difference is listed in the fidelity report as `approximated` with the original value kept in `extension.idml` so export restores it. |
| **carried** | Not interpreted. Stored unchanged under `extension.idml` at the nearest owner and written back on export where IDML allows. Listed in the report as `carried`. |
| **unsupported** | Cannot be represented and cannot be carried safely. Listed in the report as `unsupported` with part path, element, attribute and a corrective action. Never dropped silently. |

The fidelity report is JSON: `{"source": ..., "counts": {"supported": n,
"approximated": n, "carried": n, "unsupported": n}, "items": [{"status", "part",
"element", "attribute", "ref", "message", "action"}]}`. Import fails (no
output, nothing written) if the report cannot be produced.

## Package parts

| IDML part | `.pen` | Status |
| --- | --- | --- |
| `mimetype`, `META-INF/container.xml` | validated, not stored | supported |
| `designmap.xml` | `layout.spreads`, `layout.parents`, `layout.stories` order, `layout.sections` | supported |
| `Spreads/Spread_*.xml` | `layout.spreads` + `pages[]` + page nodes | supported |
| `MasterSpreads/MasterSpread_*.xml` | `layout.parents` | supported |
| `Stories/Story_*.xml` | `layout.stories` | supported |
| `Resources/Styles.xml` | `layout.styles` | supported (see below) |
| `Resources/Graphic.xml` | swatches, gradients, tints → v0.12.0 color (`color_management`) | supported; mixed inks approximated |
| `Resources/Fonts.xml` | document `fonts` + missing-font report | supported |
| `Resources/Preferences.xml` | document defaults, grids, units | approximated |
| `XML/*` (tags, backing story) | `extension.idml.xml` | carried |
| `MasterSpreads` / `Spreads` unknown elements | `extension.idml` | carried |
| `Links` folder / embedded `Link` data | `image_assets` (relinked) | supported; missing → report |
| Digital signatures, `Metadata` XMP | `extension.idml.xmp` | carried |

## Spreads, pages and parents

| IDML | `.pen` | Status |
| --- | --- | --- |
| `Spread`, `PageCount`, `BindingLocation` | spread, `facing` | supported |
| `Page` (`GeometricBounds`, `ItemTransform`, `AppliedMaster`) | page size, `layout.parent` | supported |
| `MarginPreference`, `ColumnCount`, `ColumnGutter`, `ColumnDirection` | page margins/columns | supported (RTL column direction approximated) |
| `MasterSpread` `BasedOn` | parent `based_on` | supported |
| Master item overrides (`OverrideList`, `Overridden`) | per-item per-property override | supported; unknown properties carried |
| `Section`, `PageNumberStyle`, `ContinueNumbering`, `Marker` | `layout.sections` | supported; custom marker text approximated |
| `Layer` | page layer | supported; layer `Locked/Visible/Printable` supported; `IgnoreWrap`, `SuppressTextWrap` supported |
| Page transitions, `PageItem` animation, buttons, forms | `extension.idml` | carried → `unsupported` for runtime behavior |

## Frames and page items

| IDML | `.pen` | Status |
| --- | --- | --- |
| `TextFrame` (`ParentStory`, `PreviousTextFrame`, `NextTextFrame`) | `text-frame`, `story`, `next` | supported |
| `TextFramePreference` (columns, inset, vertical justification, first baseline, auto-size, ignore wrap) | frame fields | supported |
| `Rectangle`, `Oval`, `Polygon`, `GraphicLine` | `rect`, `ellipse`, `path`, `line` or graphic frame | supported |
| Paths with `PathGeometry` / `PathPointType` | `path` data | supported (curve types exact; open/closed preserved) |
| Compound paths, `PathType` | compound path nodes | supported |
| `Group` | `group` | supported |
| `Image`, `EPS`, `PDF`, `WMF`, `PICT` placed content | `image` / vector asset | supported for raster, SVG-convertible PDF/EPS pages approximated; others → `unsupported` with relink hint |
| `ImageFitting` / `FrameFittingOption` | graphic-frame `fit` | supported |
| Clipping paths in placed images | clip + alpha | approximated |
| `TextWrapPreference` (mode, offset, side, invert, contour) | wrap | supported; contour type `DetectEdges` approximated |
| `AnchoredObjectSetting` | anchored object run | supported |
| Object style applied (`AppliedObjectStyle`) | `object` style | supported |
| Fill/stroke, tint, gradient, overprint, drop shadow, inner shadow, feather, transparency blend modes | existing fill/stroke/effects/blend + v0.12.0 overprint | supported; unmapped effects carried |
| `StrokeType` dashed, dotted, wavy, etc. | stroke style | supported for dash/solid/dot; others approximated |
| Corner options (rounded, bevel, inverse rounded, inset, fancy) | `radius` / path conversion | supported for rounded; others converted to path (approximated) |
| `Button`, `MultiStateObject`, `FormField` | none | unsupported (carried bytes preserved, not interpreted) |
| `Movie`, `Sound` | none | unsupported (out of scope: time-based media) |

## Stories and text

| IDML | `.pen` | Status |
| --- | --- | --- |
| `ParagraphStyleRange` / `CharacterStyleRange` | paragraph / run with style + overrides | supported |
| `Content`, `Br` | run text, paragraph/line break | supported |
| Special characters (`<?ACE …?>` PIs: page number, section marker, discretionary hyphen, etc.) | `special` runs | supported for the documented set; others carried |
| `HyperlinkTextSource` + `HyperlinkTextDestination` / URL destinations | hyperlink run + destination | supported (scheme allowlist applies) |
| `CrossReferenceSource`, `CrossReferenceFormat` | `xref` run | supported; custom formats approximated |
| `Footnote` (in `CharacterStyleRange`) | `note` run | supported |
| Endnote (CC 2023+ elements) | endnote story | approximated |
| `Table`, `Row`, `Column`, `Cell` | `layout.tables` | supported |
| `TextVariable` (`TextVariableInstance`) | variable run | supported for the documented set; others carried |
| `ConditionalText`, `Condition`, `ConditionSet` | condition run/set | supported |
| `Rule*`, `DropCap*`, `Bullets*`, `Tabs`, `Keep*`, `Justification*`, `Hyphenation*` | paragraph style properties | supported |
| `NestedStyle`, `LineStyle`, `GrepStyle` | nested/line/GREP | supported where the pattern fits the Pentool dialect (5.3); otherwise approximated or `unsupported` with the pattern text |
| `Composer` (Adobe Single-line / Paragraph Composer) | `composer` | approximated (Pentool composer; not byte-equal) |
| Font features (`OpenTypeFeature`) | features | supported for the documented set; others carried |
| Variable fonts (`FontStyle` axes) | axes | approximated when axis tags differ |
| Complex scripts: Arabic/Hebrew `ParagraphDirection`, `DigitsType`, `KashidaType` | direction, digits | approximated (kashida approximated) |
| East Asian: `Kinsoku`, `Mojikumi`, `Tsume`, Ruby, Warichu, tate-chu-yoko, vertical writing | none | unsupported (deferred) |
| Note (editorial `Note`), track-changes (`Change`) | none | carried |
| `Index`, `IndexSection`, `Topic`, `PageReference`, `CrossReference` entries | index markers + index | supported |
| `TOCStyle`, TOC story | `layout.generated` TOC | supported; regenerated by Pentool, original text kept |

## Styles

| IDML | `.pen` | Status |
| --- | --- | --- |
| `ParagraphStyle`, `CharacterStyle`, `ObjectStyle`, `TableStyle`, `CellStyle` | `layout.styles.*` | supported |
| `BasedOn`, `NextStyle`, `StyleGroup` | `based_on`, `next`, `group` | supported |
| Style properties with unknown/future names | `extension.idml.style.<id>` | carried |
| `NamedGrid`, `Grid` preferences | baseline/document grid | supported (`NamedGrid` approximated) |
| Style import from another IDML | `style import` | supported |

## Colors, swatches, fonts, links

| IDML | `.pen` | Status |
| --- | --- | --- |
| `Color` (RGB, CMYK, LAB; `Process`/`Spot`) | color/spot swatches | supported |
| `Tint` | tint swatches | supported |
| `Gradient`, `GradientStop` | gradient styles | supported |
| `MixedInk`, `MixedInkGroup` | none | approximated to process; report lists ink names |
| `ColorSettings` / profiles | v0.12.0 color settings | approximated by name; embedded profiles used when present |
| `FontFamily`/`Font` entries | `fonts` records | supported (missing fonts reported, substitution disclosed) |
| `Link` (`LinkResourceURI`, `StoredState`, `LinkResourceModified`) | asset link + relink report | supported (state `Missing`/`Modified` reported) |

## Export rules

- **Same value, same element:** every supported feature exports to the IDML
  element it was read from, using a deterministic ID scheme: `u` + base-36 of a
  stable counter in document order; IDs from imported IDML are reused.
- **Carried data returns:** `extension.idml` blocks are written back unchanged
  where the target element still exists; if its owner was deleted the block is
  dropped **and listed** in the export report as `carried-dropped`.
- **Unsupported on export:** a Pentool feature with no IDML equivalent (for
  example native gradient mesh or raster layers beyond IDML links) is rasterized
  or approximated only with a report entry and `--allow-approximation`;
  without it the export fails before writing.
- **Determinism:** attribute order, ID assignment, ZIP entry order and
  timestamps are fixed (entry mtime `1980-01-01`, sorted names) so bytes are
  identical across platforms.
- **Round trip:** IDML → `.pen` → IDML preserves every **supported** feature
  exactly and every **carried** block byte for byte at XML level (attribute
  order and whitespace normalized).

## Corpus

The corpus lives in `docs/fixtures/idml/` as small, intentionally authored IDML
files created for Pentool (never Adobe sample content), each with a
`<name>.expected.json` containing the expected fidelity counts, story/frame
counts, line-break lists for fonts that ship in `docs/fixtures/fonts/`, and the
unsupported items. Planned groups (each listed in
`fixtures/layout-v1-manifest.json` with its current availability):

1. `minimal-single-page`: one page, one frame, one story, two styles.
2. `threaded-columns`: three threaded frames, two columns, overset report.
3. `parents-overrides`: based-on parents, an override, a detached item.
4. `styles-inheritance`: based-on chains, nested styles, GREP style, lists.
5. `wrap-anchored`: shape wrap, jump object, inline and custom anchors.
6. `table-spanning`: a table across frames with header repeat and merged cells.
7. `notes-xrefs`: footnotes, cross-references, TOC, index.
8. `hostile`: zip-slip, bomb ratio, entity expansion, deep nesting, dangling
   references, huge counts: all must fail safely.

Acceptance (roadmap task 13): a published share of paragraphs across the
corpus matches InDesign's own PDF line breaks within the documented position
tolerance (1 LU per glyph advance sum drift, 0.5 pt per line origin) for files
whose fonts are present. The measurement script and its inputs ship with the
corpus; the number is recorded in `docs/performance.md`, never assumed.

## Implemented subset

Commands:

```sh
pentool idml-import mag.idml mag.pen --report fidelity.json [--overwrite]
pentool idml-export mag.pen mag.idml --report export.json [--allow-approximation] [--overwrite]
```

The import writes nothing unless the whole package parsed, validated and
converted; a failure leaves the output path untouched.

| Area | Import | Export |
| --- | --- | --- |
| Package | bounded ZIP (entries, ratio, size), path containment, DOCTYPE/entity refusal, XML depth and attribute limits, native `.indd`/`.indb` refusal | deterministic ZIP: stored `mimetype` first, sorted entries, fixed 1980-01-01 timestamps |
| Spreads, pages | spreads, facing pages, page size, margins, columns, gutter, sections (numbering style, start, prefix) | same elements; spread centred on its binding |
| Parents | master spreads become parents; rectangles, ovals, lines and paths render; master text frames are carried | parents become master spreads; carried master items return |
| Items | rectangle (corner radius), oval, graphic line, path/polygon, group, text frame (insets, columns, gutter), placed-graphic frame (link kept, file not embedded) | the same kinds; rotation/skew transforms and effects are reported, not written |
| Stories | paragraphs, runs, paragraph and character styles, tab, forced line break, spaces, page-number specials (`ACE 18/19/7`), frame threads (IDML links, else document order) | same, threads rebuilt from `next` |
| Styles | font, weight, italic, size, leading, justification, indents, space before/after, hyphenation, tracking, fill (character), based-on, next | the same properties; others are reported as approximated |
| Swatches | RGB/CMYK/LAB shown as sRGB fallbacks; the original value is kept in `extension.idml.swatches` and returned verbatim | swatches returned, new colors minted as RGB |
| Layers | document layers become page layers | layers written once, items reference them |
| Carried data | unknown items, item options/effects, unknown parts, unmapped style properties: stored in `extension.idml`, reported `carried` | returned at the owning element; a block whose owner is gone is `carried-dropped` |
| Fidelity report | `counts` and `items` with part, element, ref, message and action | same, plus `carried_dropped` |

Known gaps (not mapped; carried or reported, never silently lost):

- tables, footnotes/endnotes, hyperlinks, cross-references, TOC/index, text
  variables, conditions, anchored objects and inline character overrides inside
  story ranges (the story text is kept; the ranges' extra attributes are not);
- placed images are not embedded and linked files are not relinked;
- gradients, transparency, effects and tints beyond a single fill/stroke color;
- master text frames are carried but not rendered on the Pentool parent;
- the `ACE` special-character table is partial;
- unsupported or approximated export features need `--allow-approximation`;
- layer nesting and per-page layer overrides of parents are flattened.

Evidence that cannot be produced offline and is therefore not claimed: InDesign
opening the exported package, line-break match share against InDesign PDFs, and
cross-platform CI results.
