# Page layout and typesetting engine 1

Status: the current reference for format version 9 page layout. The spread,
parent, section, override, `layout-grid`, `layout-wrap`, `layout-table`, book,
IDML, tagged PDF and EPUB commands described here shipped in v0.14.0
(`docs/release-v0.14.0.md`). Text marked deferred or planned is not implemented.
Released behavior and tests win over this text; where this document and
`docs/roadmap/v0.14.0/TASKS.md` disagree, resolve it explicitly (AGENTS.md).

Companion files:

- [`layout-v1.schema.json`](layout-v1.schema.json): the `layout` object and the
  text/graphic frame nodes.
- [`pen-format-v9.schema.json`](pen-format-v9.schema.json): the document envelope
  for format version 9.
- [`penbook-v1.schema.json`](penbook-v1.schema.json): the `.penbook` manifest.
- [`idml-mapping-v1.md`](idml-mapping-v1.md): the IDML ⇄ `.pen` mapping table.
- [`fixtures/layout-v1-manifest.json`](fixtures/layout-v1-manifest.json): valid,
  invalid, adversarial and cross-platform fixtures.

## 1. Scope and principles

Pentool composes text in Rust. One composed result feeds the CLI, the editor,
the API and every export (PNG, SVG, PDF, EPUB, IDML). The browser displays and
edits what Rust composed; it never lays text out on its own.

Design rules, in priority order:

1. **Determinism.** Same document, fonts, hyphenation data and engine version
   give the same line breaks, glyph positions and page assignments on every
   release target. Section 6 defines how.
2. **No silent loss.** Overset text, missing fonts, missing links, broken threads
   and unsupported IDML content are reported with a location. They are never
   clipped, substituted or dropped without a record.
3. **Extend the existing model.** Frames are scene nodes; pages stay `pages[]`;
   edits use the shared transaction, history and planner paths. There is no
   parallel layout document.
4. **Backward compatibility.** A document without a `layout` object renders
   exactly as before. Single `text` nodes keep working (section 9).
5. **Bounded work.** Every list, depth, size and iteration count has a limit
   (section 12) checked before the expensive step.

Out of scope here (see the roadmap's *Explicitly deferred*): `.indd`/`.indb`
binaries, Adobe services, scripting, vertical CJK layout, time-based media.

## 2. Format version and migration

Layout data is **format version 9**, the next version after v8 (professional
color). The number is provisional until the first release that ships it: if
another milestone claims 9 first, this one takes the next free number and only
this section, the schema file name and `layout.engine` change.

- `version: 9` documents may contain a top-level `layout` object. A v9 document
  without `layout` is valid and renders like its v8 equivalent.
- `pentool migrate --target 9` is explicit, deterministic and validated. It
  adds `layout: {"engine": 1, ...}` with one spread per existing page and no
  parents, sections or stories. Migrating up is lossless and the result renders
  pixel-identically. Existing `text` nodes are left as they are.
- `pentool migrate --target 8` flattens layout: it fails, naming the first
  construct it cannot represent (a story, a thread, a parent, a table), unless
  `--flatten-layout` is given. Flattening composes each story and writes its
  lines as positioned `text` nodes and its parents as ordinary page items; the
  report lists every approximation. It never drops content silently.
- Versions above the supported maximum fail clearly and without mutation.
- Unknown fields on `layout`, spreads, parents, stories, frames and styles
  survive load/edit/save. They do not affect rendering.

## 3. Document structure

### 3.1 Units and coordinates

Page canvas units are those of the page (`production.units_per_inch`, default
72, so 1 unit = 1 pt). Typographic sizes (font size, leading, indents, spacing)
are stored in points and converted with `units = pt × units_per_inch / 72`.
Frame coordinates are page-local; a spread offsets its pages along x with the
documented binding gap of zero.

### 3.2 Spreads and pages

```json
"layout": {
  "engine": 1,
  "spreads": [
    {"id": "s1", "pages": ["page-1"], "facing": false},
    {"id": "s2", "pages": ["page-2", "page-3"], "facing": true}
  ],
  "page_defaults": {"margins": [54, 54, 54, 54], "columns": 2, "gutter": 12}
}
```

- `pages[]` (the existing array) remains the list of pages and their scene
  layers. A spread orders one to ten pages; each page belongs to exactly one
  spread. `facing: true` mirrors inside/outside margins on left/right pages.
- A page may carry `layout`: `parent` (parent ID), `margins`
  `[top, inner, bottom, outer]` (`[top, left, bottom, right]` when not facing),
  `columns`, `gutter`, `section` is derived, never stored.
- Page sizes, bleed and slug come from the v0.12.0 `production` object. A page
  inherits margins and columns from its parent, then from `page_defaults`.
- Spread and page order is the reading and numbering order. Reordering is a
  mutation that reports reflow (section 3.6).

### 3.3 Parents (master pages)

```json
{"id": "A-body", "based_on": null, "spread": "A", "pages": ["A-left","A-right"],
 "margins": [54,54,54,54], "columns": 2, "gutter": 12}
```

A parent owns ordinary scene nodes on its pages (guides, folios, background
frames). A page that references a parent receives those items as **parent
items**. A parent may be based on another parent; the chain has depth ≤ 16 and
no cycles.

Override rules reuse the v0.6.24 component-override contract:

- A parent item shows on the page under a derived ID `"<page>/<parent item>"`.
  It is not editable until overridden.
- An **override** records `{"item": "<parent item id>", "properties": {...}}` on
  the page: only the listed properties differ; every other property keeps
  following the parent. An override of an item that no longer exists in the
  parent is *orphaned*: reported, kept, never dropped.
- **Detach** copies the parent item onto the page as an ordinary node and
  removes the link. **Remove override** restores the parent value.
- Parent changes propagate to every page except where a property is overridden.
  A page that sets `parent: null` shows no parent items. Parent items render
  beneath the page's own items within the same layer order, then the page's
  layers.
- Text threads from a parent frame start a fresh story per page unless
  `primary: true` is set on the parent frame; a primary frame is the auto-flow
  target (section 4.4) and is overridable like any item.

#### Implementation contract (v0.14.0 task 2)

Shipped in `src/spreads.rs`; the commands are `pentool spread`, `parent`,
`section`, `override` and the extended `page` command. These choices refine the
sketch above and are the normative behavior:

- A parent stores its items in `layers` (page-shaped layers with `nodes`). A
  node may carry `parent_page` naming one of the parent's `pages` entries; it
  then shows only on the page at that position in the spread (clamped to the
  last parent page). Without `parent_page` it shows on every page.
- A derived parent replaces a base item with the same `id` in place; items of
  the base chain render first (root ancestor to leaf).
- Overrides live in `page.layout.overrides` as `{item, properties}`; `id` and
  `kind` cannot be overridden. `page.layout.detached` lists detached item IDs.
  An override or detach for an item that no longer exists is reported as
  `orphaned_overrides` by `parent resolve` and is never dropped.
- Margins resolve page, then parent chain (leaf to root), then
  `layout.page_defaults`. Facing margins are `[top, inner, bottom, outer]`;
  `parent resolve` reports absolute `[top, left, bottom, right]` plus the side
  (`left`, `right` or `single`). In a facing spread the first of two pages is
  `left`; a lone page (a cover) is `right`.
- Parent removal is refused while a page uses it or a derived parent is based
  on it (`in-use`), naming the users.
- `migrate --target 9` adds `layout` with one non-facing single-page spread per
  page and changes nothing else (pages are byte-identical). `--target 8` back
  is refused, naming the first construct, unless the layout is trivial
  (non-facing single-page spreads and no parents, sections, stories); then the
  layout object is removed.
- Version 9 documents may hold 5000 pages; earlier versions keep 1000. New
  documents and the Studio upgrade prompt stay on version 8 until the layout
  engine ships.

### 3.4 Sections and numbering

```json
"sections": [{"id": "sec-front", "start_page": "page-1", "start_number": 1,
              "style": "lower-roman", "prefix": "", "continue": false}]
```

- A section starts at a page and runs to the next section. Styles:
  `arabic`, `upper-roman`, `lower-roman`, `upper-alpha`, `lower-alpha`
  (letters repeat: 1=a … 26=z, 27=aa, 28=bb, … 52=zz, 53=aaa).
- `prefix` (≤ 16 printable characters) is prepended in the page marker.
- Markers `current`, `next` and `previous` resolve per page in text and parents;
  the first page has no previous, the last has no next (empty, reported by
  preflight as `marker-unresolved` when used in a way that shows nothing).
- Absolute page indices are 1-based in file order; numbering is computed, not
  stored. Reference vectors are in `fixtures/layout-v1-page-numbering.json`.

### 3.5 Layers

A frame lives on an ordinary page layer. Layers are not duplicated per parent;
a parent's items use the parent page's layers, merged by layer `id` and then by
name.

### 3.6 Page operations and reflow

Insert, move, duplicate and delete page are planner operations with dry run.
Each result reports `pages_added`, `pages_removed`, `stories_reflowed`,
`frames_overset` and `sections_changed`. Deleting a page that holds threaded
frames re-threads their neighbors (previous → next) and reports it; a story's
last frame is never deleted without an explicit `--drop-story`. Document-wide
reflow is bounded (section 12); exceeding the bound aborts with no mutation.

Task 2 behavior (stories arrive with task 3, so those counters stay `0`):

| Operation | Spreads | Sections and numbering |
| --- | --- | --- |
| `page add --after/--before` (`--count`, `--parent`) | each new page is its own non-facing spread; later spreads keep their IDs | later pages are renumbered; `pages_renumbered` counts existing pages whose label changed |
| `page duplicate` | copy follows the source as a single-page spread; overrides and detaches are copied | as add |
| `page move` | the page leaves its spread and forms its own | a section start moves with its page ID, then sections are re-ordered by page order |
| `page remove` | page leaves its spread (an emptied spread disappears) | a section starting there moves to the next page, or is dropped when the next page starts one; reported in `sections_changed` |
| `spread merge/split/facing/arrange` | explicit regrouping; pages are never reordered | recomputed |

Every result carries `reflow: {pages_added, pages_removed, pages_renumbered,
sections_changed, stories_reflowed, frames_overset}`; a failed or `--dry-run`
operation never writes.

## 4. Stories, frames and threads

### 4.1 Stories

```json
{"id": "feature-body", "language": "en-US",
 "paragraphs": [
   {"style": "body", "overrides": {},
    "runs": [
      {"text": "Rivers carry more than water", "style": "lead-in"},
      {"text": " — they carry the history of …", "overrides": {"italic": true}},
      {"anchor": "pull-1"},
      {"note": "fn-1"},
      {"variable": "chapter-number"}
    ]}]}
```

- A story is an ordered list of paragraphs; a paragraph is an ordered list of
  runs. A run is **exactly one of** text, anchor, note, variable, cross-reference
  (`"xref"`), index marker (`"index"`), hyperlink anchor (`"link"`),
  conditional range (`"condition"`), discretionary hyphen (`"shy"`), no-break
  range (`"nobreak"`) and special characters (`"special"`: tab, forced line
  break, column break, frame break, page break, em/en/thin/hair space, flush
  space, right-indent tab, section marker, current page number).
- Runs reference a `style` (character style ID) and may carry `overrides`:
  a property map with the same keys as the character style. Paragraphs likewise
  reference a paragraph style and carry paragraph/character overrides.
- Text is UTF-8, NFC-normalized on write. Control characters other than the
  special-character runs are refused. Language is a BCP-47 tag per story, per
  paragraph style and per run, for hyphenation and shaping.
- A story with no frames is **unplaced**: valid, reported, never dropped.
- Stories are separate from frames, so threading and reflow never rewrite text.

### 4.2 Frames

Frame nodes are scene nodes: `{"kind": "text-frame" | "graphic-frame", "id":
…}` with the ordinary v4 `transform`, `style`, `clip` and lock/visibility fields.

Text frame fields: `story`, `next` (the thread successor frame ID or `null`),
`x`, `y`, `width`, `height`, `columns` (1–40), `gutter`, `inset`
`[top, right, bottom, left]`, `vertical_align` (`top`, `center`, `bottom`,
`justify`), `vertical_justify_max_spacing`, `first_baseline` (`ascent`,
`cap-height`, `leading`, `x-height`, `fixed` + `first_baseline_min`),
`auto_size` (`off`, `height`, `width`, `both` with a reference point and
min/max), `ignore_wrap`, `baseline_grid` (`inherit`, `off`, or a frame grid),
`shape` (`rect`, `ellipse`, or a `path` reference), and `primary`.

Graphic frame fields: optional `content` (an image/vector node ID or an inline
child), `fit` (`fill`, `fit`, `fit-content-to-frame`, `fit-frame-to-content`,
`center`, `proportional`), `fit_align`, `fit_empty`.

`unassigned` frames are graphic frames without content; they may later be given
text (turning them into text frames) without changing their ID.

### 4.3 Threads

- A thread is the chain frame → `next`. The story's *thread* is the ordered
  list of frames from the one with no predecessor.
- Valid thread: every frame has the same `story`; each frame has at most one
  predecessor and one successor; no cycles; all frames exist; **no frame is
  threaded into a different story's chain**. Frames may be on different pages
  and spreads. A thread length is ≤ 4096.
- Threading, unthreading and inserting into a thread are planner operations.
  Removing the middle frame re-links neighbors by default (`--break-thread`
  keeps the tail as a new unplaced story with a derived ID `<story>-2`).
- Overset: the text that does not fit the last frame. Report: `story`, `frame`
  (last frame), `offset` (character offset in the story of the first
  non-composed character), `paragraph`, and `characters_remaining`.

### 4.4 Auto-flow

Auto-flow places a story into its thread and, while overset remains, adds a page
from the target parent with a primary frame and threads it. It is bounded by
`--max-pages` (default 100, hard maximum 5000) and by the document page limit.
It stops, reports `autoflow_truncated: true` with the remaining character count
and leaves the document consistent when the bound is hit. An empty primary frame
that does not advance the story (zero-height frame) aborts with
`autoflow-stalled` rather than adding pages.

### 4.5 Implementation notes (task 3)

- Overset is computed by a provisional, deterministic estimator until the
  composer lands: glyph advance is 0.5 x font size, line height is the
  paragraph leading (default 1.2 x size, size 12), and each frame holds
  `columns` containers. Every reported figure carries `"estimated": true`.
- Overset report: `{story, frame, offset, paragraph, characters_remaining,
  estimated}`; `tree` and `inspect` add a compact `stories` summary (counts and
  at most 20 overset entries, with `overset_truncated`).
- `unthread` and `--break-thread` split the story at the estimated overflow
  point; the tail becomes story `<story>-2` and no text is lost. `frame thread`
  of frames from different stories merges the second story onto the first.
- Removing a page re-links threaded neighbours; if a story would lose its last
  frame the command fails with `[story-last-frame]` unless `--drop-story`.
  Duplicating a page gives its frames fresh IDs and fresh empty stories.
- Story text is NFC-normalized; control characters other than newline and tab
  are refused. Frames render their composed text in PNG, SVG and PDF exports; `auto_size:
  width` is stored but not yet applied.

## 5. Styles

### 5.1 Style kinds and inheritance

`layout.styles` has `paragraph`, `character`, `object`, `table`, `cell` maps
keyed by style ID, each optionally grouped by a `group` path
(`"Headings/Display"`), with `based_on`, `next` (paragraph styles only) and
properties.

Resolution for a character in a paragraph, lowest to highest:

1. document defaults (`layout.defaults`, then built-in engine defaults);
2. the paragraph style chain (`based_on` ancestors first);
3. nested style, line style or GREP style matching (section 5.3), in document
   order of the matching rule list;
4. the character style chain of the run;
5. paragraph overrides, then run overrides.

Depth ≤ 16, no cycles; a missing `based_on` or `next` is a validation error
naming the style. Redefining a style from an overridden paragraph copies the
overrides into the style and clears them; *clear overrides* removes them.

### 5.2 Paragraph style properties (v1)

Font family/style/weight/italic/variable axes; size; leading (`auto` = 120% or
points); alignment (`left`, `center`, `right`, `justify`,
`justify-last-{left,center,right,all}`, `towards-spine`, `away-from-spine`);
indents (left, right, first-line, last-line); space before/after; same-style
spacing; tabs (position, alignment `left|center|right|decimal`, leader, align
character); drop cap (lines, characters, character style); hyphenation (on/off,
min word length, min before/after, max consecutive, zone, capitalized words,
last word, across columns); justification (word/letter/glyph min-desired-max);
composer (`single-line`, `paragraph`); keep options (together, with-next,
start-position, widow/orphan lines); span columns (`none`, `all`, `n`) with
space before/after; rules above/below; bullets/numbering (section 5.4);
align-to-grid (`none`, `first-line`, `all-lines`); language; optical margin
alignment; balance ragged lines; hanging punctuation.

### 5.3 Character, nested, line and GREP styles

Character properties: font, size, leading, tracking, kerning mode (`metrics`,
`optical`, `none`, manual units), baseline shift, horizontal/vertical scale,
skew, case (`normal`, `all-caps`, `small-caps`), position (`superscript`,
`subscript`, `ordinal`, `numerator`, `denominator`), OpenType features
(`liga`, `dlig`, `smcp`, `onum`, `lnum`, `tnum`, `frac`, `ordn`, `ss01`–`ss20`,
`calt`, `swsh`), fill, stroke, underline, strikethrough, no-break, language,
condition.

- **Nested styles** apply a character style from the start of a paragraph
  through *N* of a delimiter (character, word, sentence, tab, forced break) and
  can repeat.
- **Line styles** apply a character style to the first *N* lines.
- **GREP styles** apply a character style to matches of a pattern.

The regex dialect is a documented subset, matched by a Rust engine with a hard
step budget and **no backtracking blow-up**: literals, `.`, classes (ASCII and
Unicode general categories/scripts), anchors `^ $ \b \B`, groups, alternation,
`* + ? {m,n}` (greedy and lazy), and look-ahead only. No back-references,
look-behind, recursion, callouts or embedded code. Pattern length ≤ 512
characters; each match attempt has a budget of 100,000 steps; exceeding it
refuses the style at write time and reports `grep-budget-exceeded` at compose
time without partial application.

### 5.4 Lists

Lists (`layout.lists`) define levels, bullet or number format (`1.`, `A)`,
`i.`, custom `^#.^t`), restart policy (`after-higher-level`, `never`,
`after-story`, `after-section`) and **continuation across stories** via a
shared list ID. Numbers are computed during composition, never stored in text.

### 5.5 Object, table and cell styles

Object styles hold fill, stroke, effects, text frame options, wrap, anchored
object options, and the `fit` of graphic frames. Table styles hold borders,
alternating row/column patterns, header/footer repeat and spacing; cell styles
hold insets, alignment, rotation, fill, stroke and the paragraph style applied
to cell content.

### 5.6 Import and reflow

Styles import from another `.pen`, `.penbook` or IDML with an explicit
conflict policy (`rename`, `skip`, `replace`) and a report. A style change
recomposes **exactly** the stories (and tables, TOCs, indexes) that reference
the style through the resolution chain, found through a dependency index, in a
bounded transaction; `undo` restores the original bytes.

### 5.7 Implementation notes (task 6)

`pentool layout-style FILE <action>` (`list`, `show`, `create`, `set`, `remove`,
`usage`, `check`, `redefine`, `clear-overrides`, `import`) edits
`layout.styles.<kind>.<id>`. Every mutation validates the whole document in
memory and commits through the shared transaction; `--dry-run` and
`--if-revision` behave as for the other layout commands. Properties are
`--set key=json`. Unsupported properties, unknown `based_on`/`next` targets,
cycles and chains deeper than 16 are errors that name the style. `show`
returns the definition, the `based_on` chain and the resolved properties;
`usage` lists derived styles, `next` links and referencing stories. `remove`
refuses a style that is in use unless `--reassign STYLE` names a replacement;
derived styles otherwise fall back to the removed style's own base. `import`
remaps `based_on`/`next` within the imported set.

#### Rule evaluation and lists (task 6)

`layout-style FILE evaluate STORY` (read only, paginated) returns, per paragraph
whose resolved style has rules, ordered character-style `spans` in paragraph
character offsets (later spans win) and a `skipped` list. Order is nested, then
line, then GREP.

- `nested`: `[{"style": CHAR, "through": N, "of": DELIM, "inclusive": true,
  "repeat": false}]`. Rules apply consecutively from the paragraph start;
  `repeat: true` on the last rule cycles the whole list. `DELIM` is `character`,
  `word`, `sentence`, `tab`, `forced-break`, `em-space`, `en-space`,
  `nonbreaking-space`, `letter` or `digit`. Fewer delimiters than `through`
  styles to the paragraph end.
- `line_styles`: `[{"style": CHAR, "lines": N}]`, consecutive rules over
  consecutive composed lines. Lines exist only after composition, so without
  composed line starts these are reported as `skipped` with
  `reason: "needs-composition"`, never dropped silently.
- `grep`: `[{"style": CHAR, "pattern": "…"}]`. Patterns are compiled when the
  style is written; look-behind, back-references, named groups, inline flags,
  unsupported escapes and nested unbounded repeats such as `(a*)*` are refused,
  the last with `grep-budget-exceeded`. Single-character repeats run iteratively;
  group repeats are limited to 1,500 nested frames and 100,000 steps per match
  attempt. Overrun at evaluation time fails the whole paragraph with
  `grep-budget-exceeded` and applies no partial spans.
- A rule naming a missing character style is skipped with
  `reason: "missing-style"`.

`layout.lists` is `[{"id", "levels": [{"format", "number_style", "restart",
"start"}]}]` (1–9 levels, validated with the styles). A paragraph style joins a
list with `list: {"list": ID, "level": N}`. `layout-style FILE lists
[--story ID]` computes labels in story order; counters continue across stories
that share the list ID, a level restarts after a higher level unless `restart`
says `never`, and `after-story` restarts at each story. Format tokens: `^#`
the level's number, `^1`–`^9` an earlier level's number, `^t` a tab, `^^` a
caret. Number styles are `1`, `A`, `a`, `I`, `i`. A paragraph whose list is
undefined is reported under `problems`.

Not yet implemented: `after-section` restarts (they behave as `never` until
sections are bound to stories), composed-line evaluation of line styles in
frames, IDML style import, shortcuts.

## 6. Composition: algorithm and determinism

### 6.1 Numeric model

All composition arithmetic is **integer fixed point**: lengths are `i64`
multiples of 1/1024 pt ("layout units", LU). Font design units are scaled to LU
with round-half-away-from-zero integer division. No `f32`/`f64` value
participates in line-break decisions, glyph advances or positions; floats appear
only when converting LU to render coordinates. This removes platform and
optimizer-dependent drift.

### 6.2 Pipeline

1. Resolve styles per character (section 5.1) into style spans.
2. Segment into script/language/direction runs. Bidirectional ordering follows
   UAX #9 at paragraph level, with the paragraph direction from the style or the
   first strong character.
3. Shape each run with the OpenType shaper at the style's features and variable
   axes; keep clusters, advances in LU, and glyph IDs.
4. Find break opportunities: UAX #14 line breaking, plus discretionary hyphens,
   plus hyphenation opportunities (6.4); forced and no-break markers override.
5. Compose lines (6.3) per frame/column.
6. Position lines on the baseline grid if requested, apply keeps, widow/orphan
   control, wrap and anchored objects (section 8), tables (section 9) and notes
   (section 10), iterating to a bounded fixed point (≤ 8 passes; otherwise the
   last consistent state is used and `compose-unstable` is reported).

### 6.3 Line breaking

- **Single-line composer:** first-fit; each line is filled greedily; the
  fallback for frames/paragraphs where `composer: "single-line"`.
- **Paragraph composer (total fit):** Knuth–Plass over boxes, glue and
  penalties. Glue stretch/shrink come from the word-spacing min/desired/max;
  letter-spacing and glyph-scaling ranges add flexibility ordered after word
  spacing. For a feasible line with adjustment ratio *r*:
  `badness = min(10000, floor(100 × |r|³))` (integer cubic table), with
  `demerits = (linepenalty + badness)²` ± penalty/flag terms exactly as in the
  published algorithm. `tolerance` passes: pass 1 without hyphenation and
  tolerance 1000; pass 2 with hyphenation, tolerance 1000; pass 3 emergency,
  unbounded with an overfull-box rule.
- **Tie-breaking** is total: lowest total demerits, then the fewest lines, then
  the earliest break position. Candidate iteration order is ascending by
  position. There is no randomness and no time-based cutoff.
- Per column, each frame's column widths and wrap-adjusted line extents are
  inputs; a paragraph spanning columns/frames is composed line by line with the
  width of the column the line lands in (re-evaluated on a width change by the
  documented, bounded retry).
- Exact agreement with Adobe's composer is **not** claimed (see the roadmap).

### 6.4 Hyphenation

Liang pattern hyphenation with exception lists, implemented in Rust against
bundled, license-recorded pattern data. Each pattern set records its source,
license and checksum in `docs/hyphenation-sources.md`; only licenses compatible
with the project's license are bundled. Initial languages: en-US, en-GB, de,
fr, es, it, pt, nl, sv, da, nb, fi, pl, cs, ru, uk, el, tr. Bundled data is
≤ 1.5 MiB compressed in total. User dictionaries add words and exceptions per
language and travel with the document. Unknown languages hyphenate nowhere and
are reported as `hyphenation-unavailable`.

### 6.5 Justification, keeps and decorations

Word spacing 80/100/133 %, letter spacing 0/0/0 %, glyph scaling 100/100/100 %
by default (min/desired/max). The composer uses word spacing first, then
letter spacing, then glyph scaling, each only within its declared range. Last
line alignment follows the style. Keep options are hard constraints evaluated
after line breaking; violating a keep moves lines to the next column/frame and
recomposes the affected range. Widow/orphan is expressed as keep lines.
Drop caps occupy N lines with an indent computed from the glyph's cap height.
Optical margin alignment and hanging punctuation shift the glyph by a
documented fraction of its advance (table in the engine source, versioned with
the engine).

### 6.6 Versioning and recorded engine

`layout.engine` records the engine version integer a document was composed
with. A newer engine composes with the **recorded** behavior set unless the
user upgrades the document explicitly, which is reported and diffable by line
breaks. Behavior changes bump the engine and need a migration note and new
goldens.

### 6.7 Fonts

Font identity is family + style + weight + italic + `sha256` of the embedded or
referenced font file. Missing fonts are listed; substitution uses an explicit,
disclosed fallback chain (document fallback list, then the bundled default
face) and appears in the compose report and in preflight. Pentool never queries
the operating system's font service in compose or export, so output depends only
on documented, declared fonts (v0.6.x asset model). Subsetting and embedding
for PDF keep only the used glyphs and record the subset in the export report.

### 6.8 Implementation notes (task 4)

Engine version 1 (`shaping::ENGINE`). Shaping uses `rustybuzz` (pure-Rust
HarfBuzz port), UAX #9 via `unicode-bidi`, UAX #14 via `unicode-linebreak`,
script runs via `unicode-script` and subsetting via `subsetter`.

- `pentool text DOC shape --text T | --story ID [--font F --weight W --italic
  --size PT --features LIST --axes LIST --language TAG --direction
  auto|ltr|rtl --kerning metrics|optical|none --tracking N --baseline-shift PT
  --h-scale PCT --v-scale PCT --skew DEG --offset N --limit N]` is read-only
  and works on documents of every version. Glyph rows are
  `[glyph_id, cluster_char_offset, x_advance, y_advance, x_offset, y_offset]`
  in LU, in visual order per run, paginated (`matches`, `returned`, `offset`,
  `limit`, `has_more`). `runs`, `breaks` (character offset plus
  `allowed|mandatory`) and `missing_glyphs` are reported in full.
- Features: any four-character OpenType tag, with `-tag` to disable and
  `tag=N` for alternates; readable aliases (`ligatures`, `small-caps`,
  `oldstyle-figures`, `lining-figures`, `tabular-figures`, `fractions`,
  `ordinals`, `contextual-alternates`, `swashes`, ...). Variable-font axes are
  validated against the font's own axes.
- Tracking is 1/1000 em added after each glyph; horizontal scale multiplies
  advances; baseline shift adds to the vertical offset; vertical scale and skew
  are recorded in the report's `transform` for the renderer.
- **Optical kerning** (`--kerning optical`) disables the font's kerning and
  adjusts each adjacent pair by half the distance between the pair's summed
  side bearings and a 0.06 em target, clamped to +/-0.1 em. It is a
  deterministic approximation of outline-based optical kerning, versioned with
  the engine.
- Fonts resolve only from embedded document fonts, then the bundled default
  family; the operating system's fonts are never read. `text DOC font-report`
  lists every requested family with how it resolves; a miss is disclosed as
  `family-unavailable:<family>`, `style-substituted` or
  `weight-substituted:<from>-><to>`. A script the chosen face cannot draw
  appears as `missing_glyphs` rather than silently falling back. Embed a font
  with `pentool font DOC add ID --file FONT` (now also for version 4+ documents).
- `shaping::subset` keeps only the used glyphs plus `.notdef`; the subset
  renumbers glyphs densely and returns the old-to-new mapping. PDF embedding
  of composed frame text lands with the composer (task 5) and publishing
  exports (task 15).
- No non-Latin font is bundled to keep the binary small: Arabic, Hebrew,
  Devanagari, Thai and CJK text shapes and orders correctly when a covering
  font is embedded.

### 6.9 Implementation notes (task 5)

- `src/compose.rs` (`compose::ENGINE = 1`) implements the composer and
  `pentool text DOC compose --text T | --story ID --width PT [--style NAME]`
  (read-only; overrides `--font --size --align --hyphenate --composer
  --language`; lines are paginated with `matches/returned/offset/limit/has_more`).
- Total-fit breaking runs in integer LU with exact rational adjustment
  ratios (`badness = min(10000, floor(100 * |r|^3))`). Passes: 1 without
  hyphenation (tolerance 1000), 2 with hyphenation, 3 emergency (overfull lines
  carry a fixed 10^12 demerit penalty so they are used only when unavoidable).
  Ragged alignments receive 30 % of the measure as implicit slack so short
  lines are not scored as defects. Ties resolve to lowest demerits, then fewest
  lines, then earliest node.
- Justification spends word spacing first, then letter spacing, then glyph
  scaling; the per-line result is reported as `word_spacing_pct`,
  `letter_spacing_lu` and `glyph_scale_pct`. `justify` aligns the last line left;
  `justify-last-center|right|all` are supported.
- Hyphenation uses the bundled Liang patterns (`assets/hyphenation`, see
  `docs/hyphenation-sources.md`), the en-US exception list, and user
  dictionaries from `layout.hyphenation.exceptions`. An unbundled language is
  disclosed as `summary.hyphenation_unavailable`.
- Drop caps (`drop_cap {lines, chars}`), indents, balanced ragged lines
  (`balance`), optical margin alignment and hanging punctuation
  (small per-glyph table) and the pure `split_lines` widow/orphan/keep helper
  are implemented and unit tested.
- Composed frames render in every export (`src/typeset.rs` flows stories
  through the composer; layout preflight and `book preflight` use its overset).
- Not yet implemented: tab stops with leaders, span/split columns, character
  styles inside composition, and per-column paragraph recomposition.

## 7. Grids and guides

- A **baseline grid** is `{start, increment, relative_to: page|margin, color}`
  at document level and optional per frame; `align_to_grid: none|first-line|
  all-lines` snaps baselines by rounding **up** to the next grid line, so
  composition never moves text upward past the previous line.
- **Guides** (ruler, margin, column) and **layout grids** are persisted in
  `layout.guides`; they never render in exports.
- Align/distribute, key object, step-and-repeat and grids of frames are
  planner operations producing ordinary node transforms and IDs; step-and-repeat
  count is bounded (≤ 1000 per operation).

### 7.1 `layout-grid` (implemented, `src/grids.rs`)

All actions need a version 9 document and run through the shared validators
and transaction path (`--dry-run`, `--if-revision`).

- `baseline-set/show/clear [--frame ID]`: `layout.baseline_grid` or a frame's
  `baseline_grid` (object, or the string `off` to disable the document grid for
  that frame). `baseline-snap Y [--frame] [--page]` is read only and rounds up
  using integer milli-points, so results are identical on every platform.
  `relative_to: margin` measures from the resolved top margin of the page.
- `guide-add/set/remove`, `guide-grid`, `guide-list`: entries of
  `layout.guides` (`ruler-h` is a horizontal line at `position` y, `ruler-v` a
  vertical line at x; `layout-grid` has `rows`, `columns`, gutters and
  `relative_to`). At most 5000 guides; IDs are unique; the page must exist;
  locked guides refuse `guide-set`. `guide-list --derived --page P` appends the
  margin and column guides computed from `spread resolve`, so they are never
  stored twice.
- `measure --ids a,b,...` (2-64 objects on one page, read only): edge and center
  alignments within `--tolerance` (default 0.5 pt), pairwise gaps, equal spacing
  along each axis and the distance of every object to the margin box.
- `step-repeat ID --count N --dx --dy --prefix P`: N ≤ 1000 copies with IDs
  `P1..PN` (children get `-P<n>` suffixes). It refuses text frames (a copy would
  fork a story thread), locked layers, grouped objects and ID collisions.
- `frame-grid PREFIX --rows --columns --x --y --width --height --gutter-x
  --gutter-y [--story S]`: up to 1000 cells named `PREFIX-r<row>c<col>`,
  graphic frames, or text frames threaded in reading order through the story.
- `fit FRAME MODE [--content ID]`: `fill` (cover), `fit` (contain),
  `fit-content-to-frame` (stretch), `fit-frame-to-content`, `center`,
  `proportional` (frame width, keep aspect). The content's natural size is
  stored on the frame as `natural_size` so switching modes is idempotent.

Align, distribute, gaps and key objects remain `pentool layout`.

## 8. Text wrap and anchored objects

- Wrap modes: `none`, `bounding-box`, `shape` (object geometry), `alpha`
  (alpha channel at a threshold), `clipping-path`, `jump-object`,
  `jump-next-column`, with offsets (top, left, bottom, right), `side`
  (`both`, `left`, `right`, `towards-spine`, `away-from-spine`, `largest`) and
  `invert`.
- Wrap shapes are rasterized/flattened to at most 256 horizontal bands per
  object (tolerance in LU recorded in the engine version) and combined by
  union; the result is a set of exclusion intervals per baseline.
- Frames with `ignore_wrap: true` ignore all wrap. A wrapped object does not
  affect text in the frame that contains it as an anchored object.
- **Anchored objects** are `{"anchor": "<node id>"}` runs. Modes: `inline`
  (sits in the line, baseline offset), `above-line` (aligned, space before/
  after) and `custom` (reference point on the anchored object, on the anchor
  marker, optionally relative to column/frame/margin/page, `keep_within_frame`,
  `prevent_manual_positioning`). Anchored objects move with the text across
  frames and pages; the anchor's frame, page and offset are reported.
- Composition with wrap and anchors is bounded by 8 passes (6.2) and total
  exclusion intervals ≤ 10,000 per frame.

### 8.1 `layout-wrap` (implemented, `src/wrap.rs`)

- `set ID MODE [--offset t,r,b,l] [--side S] [--invert B] [--threshold N]`,
  `clear`, `show`, `list` store a `wrap` object on any node; locked layers and
  unknown IDs are refused and leave the file unchanged.
- `intervals FRAME [--line-height H]` is read only: per column and line, the
  free `[x0, x1]` segments of the frame after union of all wrapping objects on
  the page (never the frame itself, never objects anchored into its story,
  nothing when the frame has `ignore_wrap`). Geometry is integer milli-points.
  Rectangles use the box; ellipses use up to 256 chord bands; `alpha`,
  `clipping-path` and other kinds fall back to the bounding box and say so in
  `wrappers[].approximated`.
- `jump-object` blocks every line the object overlaps; `jump-next-column` blocks
  the column from the object's top down. `invert` keeps text inside the shape.
  More than 10,000 bands is `[limit-exceeded]`.
- Anchors: `anchor-add OBJECT --story S --paragraph N [--at CHAR]` inserts the
  anchor run (splitting text), `anchor-set`, `anchor-remove`, `anchor-list`
  and `anchor-apply`, which moves objects to the anchor's frame, page and
  layer. Results are `estimated: true` until the composer is wired to frames.
- Version 8 downgrade refuses documents with wrapped objects.

## 9. Tables

Tables are story content (`{"table": "<id>"}` run) with a definition in
`layout.tables`: rows with `height` and `min_height`, columns with widths,
cells (`row`, `col`, `row_span`, `col_span`, content paragraphs, insets,
alignment, rotation in 90° steps, strokes, fill, optional graphic content),
header/footer row counts, and a table style. Rules:

- Tables break at row boundaries; a row that does not fit goes to the next
  column/frame unless it exceeds a full frame, in which case it is split at a
  line boundary and reported. Header/footer rows repeat. Row spans keep rows
  together where possible and report where they do not.
- Merging/splitting is a planner operation that never drops cell content:
  merging concatenates content with a paragraph break in cell order.
- Text↔table conversion uses explicit row/column separators. CSV/TSV import
  follows RFC 4180, UTF-8, rows ≤ 10,000, columns ≤ 256, cell ≤ 64 KiB, total
  ≤ 8 MiB, and reports ragged rows.

### 9.1 `layout-table` (implemented, `src/tables.rs`)

- `create`, `show`, `list`, `delete`, `set` manage definitions in
  `layout.tables`; `place TABLE --story S --paragraph N [--at CHAR]` inserts
  the `{"table": id}` run. A table is placed in exactly one story and a placed
  table cannot be deleted. Limits: 1,000 tables per document, 100,000 cells per
  table.
- `cell-set` edits content (plain text; tabs become `special` runs), insets,
  alignment, rotation (0/90/180/270), strokes, fill and cell style. `merge` and
  `split` keep every cell's content (merge concatenates in cell order);
  `add-row`/`add-column` insert, `remove-row`/`remove-column` refuse to drop
  non-empty cells unless `--discard` is given.
- `from-text` converts story paragraphs (or `--text`) using explicit row and
  column separators; `to-text` reverses it without loss; `import FILE` reads
  RFC 4180 CSV/TSV (UTF-8) and reports ragged rows. Limits: 8 MiB, 10,000
  rows, 256 columns, 64 KiB per cell.
- `plan TABLE` is read only: estimated row heights and the row ranges per
  frame, with header/footer rows repeated and spans that cannot stay together
  reported. Results are `estimated: true` until the composer is wired to frames.
- A document with tables cannot be downgraded to v8.

## 10. Long-document features

- **Footnotes** are `{"note": "<id>"}` runs with bodies in `layout.notes`;
  numbering restarts per page/story/section/document; they compose at the
  bottom of the frame's column (or span columns); a note that cannot fit moves
  with its anchor line, or splits with `continues` markers, and is reported.
  **Endnotes** collect into a story at a chosen position.
- **Cross-references** carry a target (paragraph anchor, page, text anchor) and
  a format (`page`, `paragraph-text`, `paragraph-number`, `chapter`, custom
  template). Broken targets render the documented placeholder `?` and are
  reported as `xref-unresolved`.
- **Hyperlinks and bookmarks** carry destinations (URL with scheme allowlist
  `http`, `https`, `mailto`; page; text anchor); no scheme is fetched.
- **Text variables:** `chapter-number`, `running-header` (from a style, first/
  last on page), `creation-date`, `modification-date`, `output-date`,
  `file-name`, `last-page-number`, `custom`. Dates are formatted by
  locale-independent patterns and never read the wall clock during compose: the
  document's recorded `generated_at` or an explicit `--date` is used.
- **Conditional text:** conditions and condition sets (`show`/`hide`); hidden
  text is excluded from composition and kept in the story.
- **TOC, index, captions:** generated stories (`layout.generated`) record the
  inputs (styles, markers), the content digest and `stale: true` when inputs
  changed. Regeneration is explicit and deterministic and never edits user text
  outside the generated story. Index sort uses a documented collation (Unicode
  root collation tailored per language, with a code-point fallback) and page
  ranges follow `page-range` rules (`first`, `contiguous`, `all`).

### 10.1 `layout-doc` (implemented, `src/longdoc.rs`)

`pentool layout-doc DOC [--dry-run] [--if-revision N] [--file-name NAME] ACTION`
works on version 9 documents. Every action validates before writing and goes
through the shared transaction; read-only actions never write. Records live in
`layout.notes`, `bookmarks`, `hyperlinks`, `xrefs`, `variables`, `conditions`,
`condition_sets`, `index_topics` and `generated`.

- **Notes:** `note-add ID --story S --paragraph N [--at CHAR] --text T
  [--kind footnote|endnote] [--span-columns]`, `note-remove`, `note-list`,
  `note-settings [--restart document|story|section|page] [--style arabic|
  lower-roman|upper-roman|lower-alpha|upper-alpha]`, `note-plan`. Numbers are
  assigned in story reading order per the restart rule (endnotes treat `page`
  as `document`). `note-plan` reports `fits`, `splits`, `moves`, `collected`
  (endnotes) or `unplaced` per note with a footnote area of 40% of the frame
  height; all heights are `estimated: true`.
- **Cross-references:** `bookmark-add`, `link-add` (`--url` http/https/mailto
  only, `--page` or `--bookmark`), `xref-add` with `--format page|
  paragraph-text|paragraph-number|chapter|custom` and `--template` using
  `{page} {text} {number} {chapter}`. A broken target renders `?` and is listed
  by `xref-list` and `report`; nothing is dropped. `link-remove` keeps the
  visible text. URLs are never fetched.
- **Variables:** `chapter-number`, `running-header` (`--style`, `--pick
  first|last`), `creation-date`, `modification-date`, `output-date`,
  `file-name`, `last-page-number` and `custom`. Dates never use the wall clock:
  they come from `layout.created_at`, `modified_at`, `output_at`, or `--date`;
  without one the variable is reported as unresolved. `file-name` comes from
  `--file-name` or the input file name. Date patterns: `YYYY YY MMMM MMM MM DD
  HH mm ss`.
- **Conditions:** `condition-define`, `condition-set-define --show --hide`,
  `condition-activate SET|--none`, `condition-wrap`, `condition-list`.
  Hidden text is omitted from resolved text and from flow estimation.
- **Generated stories:** `generated-add ID --kind toc|index|captions|
  list-of-figures|endnotes --story S [options]`, `generate`, `generated-list`,
  `generated-check` (writes `stale`), `generated-remove`. Generated paragraphs
  use the styles `toc-N`, `index-letter`, `index-entry`, `caption-entry`,
  `figure-entry` and `endnote`. Indexes sort with a locale-tailored key
  (Swedish/Finnish å ä ö and Danish/Norwegian æ ø å follow z), group by
  letter, and apply `--page-range first|contiguous|all`; `see` topics render
  as cross-references. Captions come from paragraph styles, node `caption`
  metadata or both.
- **Staleness:** each generated record stores the SHA-256 digest of the
  paragraphs it wrote. `reason` is `never-generated`, `source-changed` (the
  document would now generate different text), `edited` (the story was changed
  by hand) or `current`. Regenerating an unchanged document is byte-identical.
- `report` lists every unresolved cross-reference, link, bookmark, variable,
  unplaced marker and stale generated item with stable codes.
- Page numbers come from the task 3 estimator, which does not yet honor
  frame-break and page-break specials; results are `estimated: true` until the
  composer is wired to frames.

## 11. Books (`.penbook`)

A `.penbook` is readable JSON listing member `.pen` paths relative to the book
file:

```json
{"format": "penbook", "version": 1, "name": "Guide",
 "documents": [{"path": "01-intro.pen", "role": "chapter"},
               {"path": "02-body.pen", "role": "chapter"}],
 "numbering": {"page": "continuous", "chapter": "continuous", "footnote": "per-document"},
 "synchronize": {"source": "01-intro.pen", "styles": true, "swatches": true,
                 "parents": false}}
```

- Member paths must stay inside the book's directory; absolute paths, `..`
  segments, drive prefixes and symlink escapes are refused. Members are ≤ 500,
  ≤ 2 GiB combined.
- Book operations (renumber, synchronize, TOC, index, preflight, package,
  export) plan across all members first and write **all or none**: members are
  staged, verified, and committed through the shared transaction/history
  mechanism with a book-level journal; any failure restores every member
  byte-for-byte.
- Book export builds one PDF (through the v0.12.0 PDF/X path) or EPUB. Missing
  members produce a report and abort; they are never skipped. PDF export is
  implemented (`pentool book <book> export out.pdf`); EPUB export is part of
  task 15 and reports `unsupported-capability` until then.
- CLI: `pentool book <book> init|status|add|remove|move|renumber|sync|toc|index|
  preflight|package|export`. Manifest edits (`init`, `add`, `remove`, `move`)
  are atomic single-file writes. Document operations first recover any
  interrupted run from the hidden `.<book>.journal/` directory (pre-images and
  `journal.json`), then stage all members, validate each, and commit through the
  shared transaction/history path; any failure restores every member byte for
  byte.
- Numbering: `renumber` continues each member's first page number from the
  previous member's last page (`page: continuous`) or restarts at 1
  (`restart-each`); the first member keeps its own numbers. Chapter and
  footnote numbers continue through `layout.book_numbering`
  (`chapter_offset`, `footnote_offset`), which `renumber` writes and which only
  shifts the chapter-number variable and (for `footnote: continuous`) document
  footnote counters.
- Synchronization copies `layout.styles` (all kinds), color swatch tokens
  (`styles` entries of type `color`, spot inks) and optionally parents from
  `synchronize.source`. The source wins: a differing local definition is
  replaced and listed as `replaced-local` in the report; styles that exist only
  in a member are kept.
- Book TOC and index are stored in one member as generated stories whose record
  has `options.scope: "book"`. They are regenerated only by `book toc` /
  `book index`; the single-document `layout-doc generate` refuses them.
  Run `book renumber` first so page labels are continuous (the result reports
  `numbering_continuous`).
- `preflight` combines per-member layout checks (overset text, unavailable
  linked images, stale generated stories, invalid or missing members) with the
  print preflight of every page when `--print` is given (capped at 200 pages).
- `package` copies the manifest, members and linked images into a new
  directory, rewriting image paths to `assets/<digest>` beside each member.

### 11.1 Data merge and adaptive layout (task 12, `src/merge.rs`)

- `layout-merge TEMPLATE --data FILE --out OUT` fills `{{field}}` placeholders
  in the stories of one template page and fills named graphic frames from an
  image-path field (`--image-field FRAME=FIELD`, paths relative to the data
  file). Data is CSV, TSV or a JSON array of flat objects, at most 16 MiB,
  10,000 records and 256 fields; a value is at most 64 KiB, and control
  characters are removed and counted in the report.
- One record per page by default; `--records-per-page N --grid COLSxROWS
  --cell WIDTHxHEIGHT` places N records per page in cells (N ≤ 1,000). The
  template is never modified; output page, frame and story IDs derive from the
  template IDs plus `m<page>` / `m<page>c<cell>`, so output is deterministic.
  The template page is removed from the output unless `--keep-template`.
- A placeholder with no data field, a missing or hostile image path, an
  anchored object inside a template story, or an ID that would collide fails
  before anything is written. `--dry-run` plans and reports without writing.
- The report lists `overflow` per record with page, story, frame and
  `characters_remaining`. Overflow is **estimated** (marked `estimated: true`);
  run `text overset` on the output for composed truth.
- `layout-adapt FILE --page P --width W --height H --rule scale|recenter|guide|object`
  resizes one page and re-places its objects: `scale` (`--uniform` keeps the
  aspect ratio and centers), `recenter` (keep size, keep the center), `guide`
  (objects keep their distance to the nearest guide, which moves
  proportionally) and `object` (per-object `--object ID=h:left|right|center|
  stretch|scale,v:top|bottom|center|stretch|scale`). Text size is unchanged.
- `layout-alt FILE create SOURCE ID --width W --height H` copies a page to a new
  size (same rules). Each copied story records `linked_to` and `link_digest`.
  `sync` refreshes linked stories from their sources; a story whose digest no
  longer matches was edited locally and is skipped and reported unless
  `--force`. `unlink PAGE` drops the link, `list` is read only. At most 500
  alternates.
- All three require a version 9 document and commit through the shared
  transaction path (`layout-merge` writes a new file atomically).

### 11.2 `layout-export` (implemented, `src/publish.rs`)

`pentool layout-export FILE OUT [--format pdf|epub] [--fixed] [--scale N]
[--title T] [--language BCP47] [--overwrite]` composes every placed story with
the same engine as rendering and preflight, then writes:

- **PDF** — page artwork is a raster of the page rendered *without* story
  text; composed lines are real text using embedded font subsets (Type 0,
  Identity-H, ToUnicode) placed from shaped advances, justified lines with
  per-space adjustment. The file is tagged (`MarkInfo`, `StructTreeRoot`,
  `Document`/`H1`-`H6`/`P`, `Figure` with `Alt` from `alt`, `alt_text` or
  `description`), has bookmarks from paragraph styles named like `heading-N`,
  `title`, `chapter` or `hN`, URL and page/bookmark link annotations from
  `layout.hyperlinks` link runs, `/Lang` and a document title.
- **EPUB 3** — reflowable by default: one XHTML file per placed story with
  headings, paragraphs, link runs, per-style CSS, embedded fonts and a
  navigation document. `--fixed` writes pre-paginated pages: page artwork as a
  PNG with positioned text. Identifiers are content-derived and the modified
  date is fixed so output is deterministic.
- `book export --format epub` joins the members in order into one EPUB.

Not covered: PDF/X real-text output (use `export-pdfx`), PDF articles, right-to-left
placement, images in reflowable EPUB, and validator runs (veraPDF, EPUBCheck).

### 11.3 `layout-preflight` (implemented, `src/layout_preflight.rs`)

Read-only. Issues carry `severity`, `code`, `message`, `action` and, where
they apply, `page`, `frame`, `story`, `offset`: errors `text-overset`,
`link-missing`, `text-outside-bleed` and unresolved references; warnings
`font-substituted`, `link-modified`, `generated-stale`, `frame-empty`,
`pasteboard-object`, `alt-text-missing`, `style-overrides`, `note-unused`.
At most 500 issues are listed (`issues_total` and `truncated` say the rest).

## 12. Limits

| Limit | Value |
| --- | --- |
| Pages per document | 5,000 |
| Pages per spread | 10 |
| Parents / parent chain depth | 500 / 16 |
| Stories / paragraphs per story / runs per paragraph | 10,000 / 100,000 / 10,000 |
| Characters per story / per document | 16 MiB / 64 MiB |
| Thread length | 4,096 frames |
| Frames per page | 10,000 |
| Style count per kind / `based_on` depth | 2,000 / 16 |
| Tables per document / cells per table | 1,000 / 100,000 |
| Footnotes per document | 100,000 |
| GREP pattern length / step budget | 512 / 100,000 per attempt |
| Compose passes | 8 |
| Reflow per edit (affected paragraphs) | 50,000, else aborted before mutation |
| IDML archive size / entries / uncompressed / ratio | 512 MiB / 20,000 / 2 GiB / 100:1 |
| IDML XML depth / attributes per element | 128 / 256 |
| EPUB archive entries / uncompressed | 20,000 / 2 GiB |
| Font file / glyphs processed per compose | 64 MiB / 50 M |
| CSV/TSV import | 8 MiB, 10,000 rows, 256 columns |
| Autoflow pages per operation | 100 default, 5,000 maximum |

Every limit is checked before the expensive step and fails with an error naming
the limit, the actual value and the location.

## 13. Reports and diagnostics

Compose, import, export and preflight use one structured result shape:
`{"engine": 1, "ok": bool, "counts": {...}, "issues": [{"code", "severity",
"page", "frame", "story", "offset", "node", "message", "action"}]}`. Codes are
stable and documented. `tree` and `search` show stories, threads and overset
compactly (IDs, kinds, hierarchy, bounds, counts) and never dump story text;
`matches`, `returned`, `offset`, `limit` and `has_more` describe the emitted
result exactly.

## 14. Threat model

Untrusted inputs: IDML and EPUB archives, `.pen`/`.penbook` files, fonts, CSV/
TSV and JSON for data merge, hyphenation dictionaries, regex style patterns,
hyperlink targets.

| Threat | Mitigation |
| --- | --- |
| Zip slip, absolute paths, symlinks, drive prefixes | Reject entries whose normalized path is absolute, contains `..`, a drive, a NUL or a backslash escape; never extract to disk, read members in memory; `.penbook` paths contained in the book directory |
| Zip bombs | Entry count, per-entry and total uncompressed size and compression ratio are checked from the central directory **and** enforced while streaming |
| XML entity expansion, XXE, DTD fetch | DTDs and external entities are refused; the parser never resolves a URI; node count, depth and attribute counts are bounded before building a tree |
| Hostile fonts | Size cap; parse failures are per-font and non-fatal; glyph, table and instruction budgets; no hinting bytecode is executed |
| Catastrophic regex | Restricted dialect, no back-references, step budget (5.3) |
| Quadratic or unbounded layout | Section 12 limits, bounded passes, reflow caps, cancellation checks between paragraphs/pages and between import/export stages |
| Resource exhaustion in tables/wrap | Cell, band and interval caps (8, 9) |
| Hostile dimensions | Finite, bounded, non-negative checks; NaN/∞ refused; page and frame sizes capped at 1,000,000 units |
| Network | No scheme is fetched during open, compose or export; links are data |
| Partial writes | Everything is built in memory and validated before the shared transaction writes; cancellation and crash recovery restore the original bytes |
| Output injection | IDML/EPUB/PDF writers escape all text and attribute values and validate IDs |

## 15. Dependencies

The AGENTS.md bar: product benefit, compatible license, maintained upstream,
bounded behavior, binary size and supply-chain cost.

| Need | Decision | Why |
| --- | --- | --- |
| OpenType shaping | `rustybuzz` (already in the tree through `resvg`'s text stack) | Pure Rust HarfBuzz port, maintained, deterministic given inputs; no new dependency |
| Bidi (UAX #9) | `unicode-bidi` (already in the tree) | Spec implementation, already audited by the build |
| Scripts/vertical orientation | `unicode-script`, `unicode-vo` (already in the tree) | Already present |
| Line breaking (UAX #14) | Evaluate `unicode-linebreak` (MIT/Apache, small, pure Rust) against writing the table lookup in-tree at task 4; record the choice and size delta in `docs/architecture.md` | Needed for CJK and general breaks; keep only if size cost is within budget |
| Hyphenation | In-tree Liang implementation + bundled pattern data | Tiny algorithm, avoids a dependency, data is the only size cost (≤ 1.5 MiB budget) |
| XML reading (IDML, EPUB) | `roxmltree` (already in the tree) | No DTD/entity expansion beyond bounds, node limit option, read-only |
| XML writing | In-tree escaping writer | Small and fully controlled |
| ZIP | `zip` (already a dependency) | Already used by packages; add hostile-archive checks from section 14 |
| Font subsetting for PDF | Decision at task 4: in-tree glyf/CFF subsetter versus a maintained pure-Rust subsetter crate | Needs binary-size and correctness evidence; not decided here |
| Validators (veraPDF, EPUBCheck) | CI/test only, never linked or shipped | Conformance evidence, not a runtime dependency |

No Adobe SDK, no network library and no system font service is introduced.

## 16. Required evidence per task

Each roadmap task needs, before its checkbox is ticked: the implementation, the
fixtures named in `fixtures/layout-v1-manifest.json` updated, tests in the
owning module and in `tests/`, CLI help and README examples kept runnable,
cross-platform golden digests for composition, and a note in this file or the
roadmap when a deliberate decision changes the contract.
