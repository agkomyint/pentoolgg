# v0.14.0 — Professional page layout and InDesign interchange

Bring Pentool's page layout to the level of Adobe InDesign. The goal is feature
parity for real professional publishing work, such as magazines, books, reports,
catalogs and packaging copy, plus faithful exchange with InDesign through IDML.

A designer moving from InDesign should find every core concept they rely on:
- threaded text frames, parent pages, and paragraph, character, object, table
  and cell styles;
- a paragraph composer, baseline grids, text wrap, anchored objects and tables;
- footnotes, tables of contents, indexes, cross-references and books.

A designer should be able to open an IDML file in Pentool, edit it, and send it
back as IDML.

"1:1" means equivalent capability and faithful file exchange, not cloning
Adobe's product identity. Pentool keeps its own interface, icons, names and
documentation, and uses no Adobe code, trademarks, artwork or proprietary
resources. Interoperability goes through IDML, Adobe's published XML
interchange format. Native `.indd` files are binary and undocumented; Pentool
refuses them with a clear message asking for IDML instead (see Explicitly
deferred).

Print production comes from v0.12.0: ICC color, CMYK, spot inks, overprint,
bleed and slug, PDF/X and preflight. This release builds on it rather than
re-implementing it.

This is a minor release because it adds format data, file types, commands and
editor features. Patch releases in the v0.14 line remain limited to bug fixes
and documentation.

## Product contract

- The Rust typesetting engine is authoritative. Shaping, line breaking,
  hyphenation, justification, frame flow, footnotes and pagination run in Rust.
  The CLI, editor, API and every export (PNG, SVG, PDF, EPUB, IDML) share one
  composed result. The browser never lays out text independently; it displays
  and edits what Rust composed.
- Composition is deterministic. The same document, fonts, hyphenation patterns
  and engine version give the same line breaks, glyph positions and page
  assignments on Linux, Windows, Intel macOS and Apple Silicon. The layout
  engine is versioned, and a document records the engine version it was
  composed with.
- Text is stored as structured stories, separate from the frames that display
  them. A story is readable JSON: runs, paragraphs and style references. Frames
  reference a story and their place in its thread.
- Overset text, missing fonts, missing links and broken threads are reported
  explicitly. They are never silently clipped, substituted or dropped.
- Layout edits are ordinary mutations. Dry run and commit share one planner, and
  failed, cancelled, stale or locked edits leave the document byte-for-byte
  unchanged. Reflow caused by an edit is bounded and reported.
- IDML import never silently loses content. Every element, attribute and
  setting Pentool does not support is listed in a fidelity report with its
  location. Supported content round-trips through IDML without change.
- IDML, EPUB and hostile font input are untrusted. ZIP paths are contained, XML
  has no external entities or DTD fetching, and sizes, counts, nesting depth and
  decompression ratios are bounded before parsing.
- Existing documents render exactly as before unless they use a v0.14.0
  feature. Pentool's current single-frame text keeps working; it becomes a story
  with one frame.
- No network, no Adobe software, and no system font service is required to open,
  compose, export or verify a document.

## Layout model

```json
{"stories": [{"id": "feature-body", "paragraphs": [
   {"style": "body", "runs": [{"text": "Rivers carry more than water", "style": "lead-in"},
                              {"text": " — they carry the history of …"}]}]}],
 "frames": [{"kind": "text-frame", "id": "body-1", "story": "feature-body",
             "x": 54, "y": 120, "width": 504, "height": 600, "columns": 2,
             "gutter": 12, "inset": [0, 0, 0, 0], "next": "body-2"}],
 "parents": [{"id": "A-body", "based_on": null, "spread": "…"}],
 "styles": {"paragraph": {"body": {"based_on": "base", "font": "Source Serif 4",
   "size": 10, "leading": 13, "align": "justify", "hyphenate": true,
   "composer": "paragraph", "align_to_grid": "all-lines"}}}}
```

- Documents have spreads of one or more pages, facing or single, with page
  sizes, margins, columns, bleed and slug from v0.12.0, sections, and page
  numbering styles (arabic, roman, alphabetic, with prefixes).
- Parent pages, based on other parents, supply items to pages. Local overrides
  are tracked per item and per property, and parent changes propagate except
  where an item has been overridden. This reuses the component-override rules
  from v0.6.24.
- Stories flow through threaded frames. A frame may be a text frame, a graphic
  frame, or unassigned. Text on a path remains a frame kind.
- A book is a new file type, `.penbook`: a readable JSON manifest that lists
  `.pen` documents in order. It defines continuous numbering, synchronized
  styles and swatches, and book-wide TOC, index and export.

## Must ship, in order

- [ ] **1. Freeze the layout specification, IDML mapping and threat model.**
  Specify:
  - spreads, pages, parents and overrides, sections, stories, threads and frame
    options;
  - the style model and inheritance;
  - the composer, hyphenation and justification algorithms, the baseline grid,
    wrap, anchored objects, tables, footnotes and long-document features;
  - the `.penbook` file type, and the format version with its migration.

  Publish a full IDML ⇄ `.pen` mapping table, listing supported, approximated
  and unsupported features with how each is reported. Publish schemas plus
  valid, invalid, adversarial and cross-platform fixtures, including a corpus
  of IDML files with known expected output. Each new dependency (shaping,
  bidi, line breaking, XML) must be justified against the AGENTS.md bar.
  *Progress:* specification (`docs/layout-v1.md`), IDML mapping
  (`docs/idml-mapping-v1.md`), schemas (`layout-v1`, `pen-format-v9`, `penbook-v1`),
  threat model, dependency decisions and valid/invalid/adversarial/cross-platform
  fixtures are published and pinned by `tests/v0140_layout_spec.rs`. Still open
  before this box is ticked: the authored IDML corpus with expected output
  (`docs/fixtures/idml/`, groups listed in the manifest) and composition goldens,
  which need the importer and engine.

- [ ] **2. Add documents, spreads, parents and sections.**
  - Facing and single pages, spreads, page sizes, margins and columns per parent
    or page.
  - Parent pages based on parents, with per-item, per-property override and
    detach.
  - Sections with numbering styles, and current, next and previous page-number
    markers.
  - Inserting, moving, duplicating and deleting pages, with documented reflow
    behaviour.

  Migrating up is lossless. Existing pages become single-page spreads with no
  parent.

  *Progress:* spreads, parents (based-on chains, per-item per-property override,
  detach, orphan reporting), sections with numbering and page labels, and page
  insert/move/duplicate/delete with a reflow report are implemented in
  `src/spreads.rs` and covered by `tests/v0140_spreads.rs`; contract in
  `docs/layout-v1.md` section 3.3/3.6. Still open before this box is ticked:
  per-page size/margin editing commands, the `current`/`next`/`previous` text
  markers (need stories from task 3), Studio surfaces, and cross-platform CI
  evidence.

- [ ] **3. Add stories, threaded frames and frame options.**
  - Stories are separate from frames: thread, unthread, insert into a thread,
    and auto-flow text, which adds pages from a parent in a bounded way.
  - Overset detection and reporting.
  - Frame options: columns, gutter, inset, vertical justification, first
    baseline offset, auto-size, and ignore wrap.

  `text story` and `text frame` commands, and tree and search report stories,
  threads and overset text compactly.

  *Progress:* stories, threaded frames, frame options, bounded auto-flow and
  overset reporting are implemented in `src/stories.rs` (`text story` and
  `text frame` commands, tree/search/page-operation reporting) and covered by
  `tests/v0140_stories.rs`; contract in `docs/layout-v1.md` section 4.
  Overset figures come from a provisional estimator (every figure carries
  `"estimated": true`) until the composer in task 4 replaces it; frames render
  nothing until then. Still open before this box is ticked: `width` auto-size
  composition, Studio surfaces, and cross-platform CI evidence.

- [ ] **4. Build the shaping and font engine.**
  - OpenType shaping for Latin, Greek, Cyrillic, Arabic, Hebrew, Devanagari,
    Thai and CJK, through an audited pure-Rust shaper.
  - Bidirectional text and Unicode line-break opportunities.
  - OpenType features: ligatures, kerning, optical kerning, small caps,
    old-style and lining figures, tabular figures, fractions, ordinals,
    stylistic sets, contextual alternates and swashes.
  - Tracking, baseline shift, horizontal and vertical scale, skew, and variable
    font axes.
  - Font embedding and subsetting for PDF, and a missing-font report with
    substitution disclosed.

  *Progress:* the shaping engine (`src/shaping.rs`: rustybuzz shaping, UAX #9
  bidi, UAX #14 break opportunities, features, axes, tracking, scale, optical
  kerning, font resolution with disclosed substitution, subsetting) and the
  `text shape` / `text font-report` commands are implemented and covered by
  `tests/v0140_shaping.rs`; contract in `docs/layout-v1.md` section 6.8. Still
  open before this box is ticked: PDF embedding of subset fonts (needs the
  composer), shaping coverage against embedded Arabic/Devanagari/Thai/CJK
  fonts (none bundled), vertical text, and cross-platform CI evidence.

- [ ] **5. Build the paragraph composer.**
  - A total-fit line breaker in the Knuth–Plass family, plus a single-line
    composer.
  - Hyphenation from bundled, license-compatible patterns for the documented
    languages, with dictionaries, discretionary and no-break markers, and zones.
  - Justification settings: word and letter spacing, glyph scaling with
    minimum, desired and maximum values, and last-line alignment.
  - Keep options: with next, together, start position, widow and orphan control.
  - Drop caps, balanced ragged lines, optical margin alignment, and
    hanging punctuation.
  - Span and split columns, tab stops with leaders, and indents (left, right,
    first-line, last-line).

  Golden tests pin line breaks across the IDML corpus.

  *Progress:* the composer (`src/compose.rs`: Knuth-Plass total-fit and
  single-line breaking, Liang hyphenation from bundled patterns, justification
  across word/letter/glyph scaling, indents, drop caps, balanced ragged lines,
  optical margins, hanging punctuation, widow/orphan/keep arithmetic) and
  `text compose` are implemented and covered by `tests/v0140_compose.rs`;
  contract in `docs/layout-v1.md` section 6.9. Still open before this box is
  ticked: tab stops with leaders, span and split columns, last-line indent
  edge cases, goldens across the IDML corpus, wiring the composer into frames
  (replacing the estimator), and cross-platform CI evidence.

- [ ] **6. Add styles.**
  - Paragraph, character, object, table and cell styles, with `based_on`
    inheritance, a next style, style groups, and the shortcuts, clearing and
    redefining of overrides.
  - Nested styles, line styles, and GREP-equivalent regex styles. These use a
    documented, bounded regex dialect, not arbitrary code.
  - Style import from another `.pen` file or from IDML.
  - Bulleted and numbered lists with levels, restart rules and list
    continuation across stories.

  Style changes reflow every affected story in a bounded way.

  *Progress:* the style core (`src/styles.rs`, `pentool layout-style`) is
  implemented and covered by `tests/v0140_styles.rs`: the five style kinds with
  per-kind property validation, `based_on` chains (depth <= 16, cycle and
  missing-base errors), next styles, groups, resolution, a dependency/usage
  report, create/set/remove (with `--reassign`), redefine from overrides,
  clearing overrides, and import with `rename`/`skip`/`replace` policies from a
  version 9 `.pen`; contract in `docs/layout-v1.md` section 5.7. Still open
  before this box is ticked: composed-line evaluation of line styles in
  frames, `after-section` list restarts, IDML style import, shortcuts, and
  wiring style resolution into frame composition so a change recomposes the
  affected stories. Nested, GREP (`src/style_eval.rs`, bounded dialect with a
  100,000-step budget) and list evaluation (`layout-style evaluate` / `lists`)
  are implemented and covered by `tests/v0140_style_eval.rs`.

- [ ] **7. Add grids, guides and alignment.**
  Progress: `layout-grid` (`src/grids.rs`, `tests/v0140_grids.rs`) provides
  baseline grids (document and frame), ruler/layout-grid guides with derived
  margin and column guides, smart-guide measurement, step-and-repeat, grids of
  frames and graphic-frame fitting; align/distribute/key object already exist
  in `pentool layout`. Open: applying `align_to_grid` to composed lines,
  content-aware (alpha/subject) fitting, studio overlays, CI evidence.
  - A baseline grid per document and per frame, with align-to-grid modes.
  - Ruler guides, margin and column guides, layout grids, and smart guides with
    measurement.
  - Align and distribute, including spacing distribution and key objects.
  - Content-aware fitting for graphic frames.
  - Step-and-repeat, and grids of frames.

- [ ] **8. Add text wrap and anchored objects.**
  Progress: `layout-wrap` (`src/wrap.rs`, `tests/v0140_wrap.rs`) sets, clears,
  shows and lists node `wrap`, computes per-line free segments of a frame
  (`intervals`: bounding box, ellipse shape bands, side/spine, invert, jump
  object, jump to next column, `ignore_wrap`), and adds, edits, removes, lists
  and applies anchored objects (inline, above-line, custom). Positions come
  from the task 3 estimator and are reported `estimated: true`; downgrade to v8
  refuses wrapped objects. Open: wrap in the composer, alpha-channel and
  clipping-path shapes (reported as bounding-box `approximated`), CI evidence.
  - Wrap around a bounding box, an object shape, an alpha channel or a clipping
    path; jump object and jump to next column; offsets, sides and invert.
  - Inline, above-line and custom anchored objects, with reference points,
    keep-within-frame, and movement with text across frames and pages.

  Composition with wrap and anchors is deterministic and bounded.

- [ ] **9. Add tables.**
  Progress: `layout-table` (`src/tables.rs`, `tests/v0140_tables.rs`) creates,
  edits, merges, splits and places tables; adds and removes rows and columns
  without silently dropping content; converts text to a table and back;
  imports RFC 4180 CSV/TSV within the documented limits; and plans row breaks
  with repeated header and footer rows. Heights come from the task 3 estimator
  (`estimated: true`). Open: composer-driven row heights and rendering,
  graphic cell content, IDML table mapping, CI evidence.
  - Rows and columns, merging and splitting, and header and footer rows that
    repeat when a table breaks across frames and pages.
  - Cell insets, alignment and rotation, strokes and fills, alternating
    patterns, and graphic cells.
  - Table and cell styles, and conversion between text and tables.
  - Import from CSV or TSV with a bounded size.

  Tables flow inside stories and break predictably.

- [ ] **10. Add long-document features.**
  Progress: `layout-doc` (`src/longdoc.rs`, `tests/v0140_longdoc.rs`, design in
  `docs/layout-v1.md` section 10.1) provides footnotes, endnotes and
  column-spanning notes with numbering rules and a fit plan; bookmarks,
  hyperlinks (http/https/mailto only) and cross-references with formats;
  variables (running header, dates without the wall clock, file name, chapter
  number); conditions and sets; and deterministic generated TOC, index,
  captions and endnote stories with digest-based stale state. Page numbers use
  the task 3 estimator (`estimated: true`). Open: composer-driven note
  placement and rendering of generated items, frame-break specials in the
  estimator, studio UI, CI evidence.
  - Footnotes, endnotes, and column-spanning notes.
  - Cross-references with formats, and hyperlinks and bookmarks.
  - Text variables: running headers from styles, date, file name and chapter
    number.
  - Conditional text with condition sets.
  - Tables of contents from styles; indexes with topics, page ranges and
    cross-references; captions from metadata.
  - Every generated item is regenerated deterministically and reports stale
    state when the document changes.

- [ ] **11. Add books (`.penbook`).**
  - Ordered documents with continuous or restarting page, chapter and footnote
    numbering.
  - Style and swatch synchronization from a source document.
  - Book-wide TOC, index, preflight, packaging and export to one PDF or EPUB.

  Book operations are transactional across member documents: either every
  member is written or none is.

  Progress: `pentool book` (`src/book.rs`, `tests/v0140_book.rs`,
  `docs/layout-v1.md` section 11) provides init/add/remove/move, status,
  renumber (continuous or restart-each pages, chapter and footnote offsets),
  style/swatch/parent synchronization, book-wide TOC and index, preflight,
  package and one-file PDF/X-4 export, with a journaled all-or-none commit.
  Open: EPUB export (task 15), InDesign `.indb` import (task 13), cross-platform
  CI evidence, Studio UI (task 17).

- [ ] **12. Add data merge and adaptive layout.**
  Progress: `pentool layout-merge`, `layout-adapt` and `layout-alt`
  (`src/merge.rs`, `tests/v0140_merge.rs`, `docs/layout-v1.md` section 11.1)
  cover CSV/TSV/JSON merge into text and image placeholders (one or several
  records per page, estimated overflow report), scale/recenter/guide/object
  adaptation and linked alternate layouts. Open: composed (not estimated)
  overflow in the merge report, Studio UI (task 17), cross-platform CI
  evidence.
  - Data merge from bounded CSV or JSON into text and image placeholders, giving
    one record per page or several, with an overflow report.
  - Layout adjustment rules for page size changes: guide-based, scale, recenter
    and object-based.
  - Alternate layouts, such as print and tablet sizes, that share linked
    stories.

- [ ] **13. Import IDML.**
  Progress: `pentool idml-import` (`src/idml.rs`, `tests/v0140_idml.rs`,
  fixture `docs/fixtures/idml/minimal/`, subset table in
  `docs/idml-mapping-v1.md`) reads spreads, pages, parents, stories, styles,
  swatches, layers, shapes, groups, text frames and threads with bounded,
  contained package reading and a fidelity report. Open: tables, footnotes,
  anchored objects, variables, conditions, xrefs, TOC/index, image relinking,
  `.penbook` from book files, the corpus and its measured line-break share, CI
  evidence.
  - Spreads, pages, parents, stories, styles, swatches (through v0.12.0's
    color), layers, frames, threads, tables, footnotes and anchored objects.
  - Links to placed images (relinked through the existing asset model), text
    variables, conditions, cross-references, TOC styles and the index.

  Produces a `.pen` (or a `.penbook` for an InDesign book file exported to IDML
  per document) plus a fidelity report. Hostile archives are refused safely.
  Acceptance: a measured share of the corpus matches InDesign's own PDF of each
  file within documented line-break and position tolerances.

- [ ] **14. Export IDML.**
  Progress: `pentool idml-export` (`src/idml_export.rs`) writes the same subset
  deterministically, returns carried blocks (reporting `carried-dropped`) and
  fails without `--allow-approximation` when a feature has no IDML equivalent.
  The IDML → `.pen` → IDML round trip of the minimal fixture is covered by a
  test. Open: the remaining mapping rows, opening files in current InDesign
  versions (release evidence), line-break parity, CI evidence.

  Write IDML that opens in InDesign with:
  - stories, styles, threads and parents intact;
  - text that reflows to the same lines where fonts match.

  An IDML file → `.pen` → IDML round-trip preserves every supported feature
  exactly and carries unsupported feature data unchanged where the format
  allows. Record evidence of the file opening in current InDesign versions as
  release evidence.

- [ ] **15. Add publishing exports.**
  Progress: `pentool layout-export FILE OUT [--format pdf|epub] [--fixed]`
  and `book export --format epub` (`src/publish.rs`,
  `tests/v0140_publish.rs`, `docs/layout-v1.md` section 11.2) write a tagged
  PDF with real, searchable text (embedded font subsets, ToUnicode), a
  structure tree with heading roles and figure alt text, bookmarks from
  heading styles, URL and internal link annotations, and EPUB 3 reflowable
  (story text, paragraph styles, navigation document, embedded fonts) and
  fixed-layout (pre-paginated, positioned real text over page artwork).
  Open: PDF/X path with real text (the v0.12.0 flattened draft stays raster),
  PDF articles/threads, Link structure elements, right-to-left and
  complex-script placement, character-style application in composition,
  veraPDF and EPUBCheck validation in CI, images in reflowable EPUB.
  - Interactive PDF with links, bookmarks and articles.
  - Print PDF through v0.12.0's PDF/X path, with composed text kept as real,
    selectable and searchable text.
  - Tagged PDF with a reading order and alternative text toward PDF/UA.
  - EPUB 3 in both reflowable form (from styles and reading order) and fixed
    layout.

  Every export passes its validator in CI: veraPDF for PDF/UA and PDF/X,
  EPUBCheck for EPUB. The validators are used in tests only.

- [ ] **16. Extend preflight for layout.** Extend v0.12.0 preflight with:
  - overset text, missing and substituted fonts, and missing, modified or
    low-resolution links;
  - unresolved cross-references and stale TOC or index entries;
  - empty frames, text outside the bleed, and objects on the pasteboard;
  - style overrides beyond a threshold, and missing alt text.

  Results are structured JSON with page, frame, story and offset, plus a
  corrective action. Live preflight runs in the editor.

  Progress: `pentool layout-preflight FILE [--bleed PT] [--max-overrides PCT]
  [--no-alt]` (`src/layout_preflight.rs`, `docs/layout-v1.md` section 11.3)
  reports composer-measured overset, substituted fonts, missing or modified
  links, unresolved references, stale generated stories, empty frames, text
  outside the bleed, pasteboard objects, style overrides and missing alt text
  as structured JSON with page, frame, story, offset and action. `book
  preflight` uses the composed overset count. Open: low-resolution link check
  against placed size, live preflight in the editor (task 17), CI evidence.

- [ ] **17. Build the editor's layout experience.**
  - A pages panel with spreads, parents and drag-to-apply, and a story editor.
  - Type tools with frame threading by click, plus loaded-cursor placement of
    text and images.
  - Panels for paragraph, character, object, table and cell styles, with
    override indicators.
  - Overlays for the baseline grid, guides, frame edges, hidden characters and
    overset text.
  - A text-wrap UI, anchored-object handles, and table editing.
  - Interfaces for TOC, index and cross-references; the links panel; and a
    preflight panel.
  - An optional keyboard-shortcut set familiar to InDesign users.

  Progress (partial, checkbox left open): the Studio Layout workspace has a
  pages panel (spreads, facing toggle, parents, add/duplicate/move/remove), a
  story editor with frame creation, threading by click and flow, a frame panel,
  a styles panel with override counts, overlays for frame edges, thread links,
  overset markers, margins and columns, a preflight panel with clickable issues,
  tagged PDF and EPUB downloads, and an opt-in InDesign key set (V, T,
  Page Up/Down, Ctrl+Shift+P). Every edit goes through `POST /api/layout`, so
  the planners are the Rust ones and each op is one undo step. Also built:
  table editing (create, cell grid, rows/columns, merge/split, header rows,
  place), text-wrap and anchored-object controls, TOC/index/cross-reference and
  links panels with stale flags, style panels for all five kinds with property
  editing, baseline-grid, guide and hidden-character overlays, and
  loaded-cursor placement of text files (click the canvas to place). Not built:
  loaded-cursor placement of images, on-canvas anchor handles, and incremental
  recomposition (every edit recomposes the whole document, 616 ms for 500
  pages cold and about 20 ms when cached, so typing is not yet per-range).

  Pentool's visual design remains its own. Typing and editing stay responsive
  on a 300-page document: Rust recomposes only the affected story range, and
  the browser receives composed lines incrementally.

- [ ] **18. Harden conformance, performance and failure recovery.**
  - Golden composition and render tests on all four release targets.
  - Fuzzing of IDML, EPUB, fonts, regex styles, tables and hostile dimensions.
  - Cancellation at every import, compose and export stage; transaction
    rollback, including for books; and crash recovery.
  - Benchmarks for composing and exporting a 500-page book, recorded in
    `docs/performance.md`.

  Progress (partial, checkbox left open): the 500-page compose and export
  benchmark is recorded in `docs/performance.md`. `scripts/layout-smoke.py`
  imports IDML, preflights, exports PDF and EPUB, exports IDML and re-imports
  it; it runs in the release workflow. It found and fixed a real bug: pages
  with float canvas sizes (as IDML import writes them) failed to render.
  Golden composition on all four targets, fuzzing, cancellation at every stage
  and crash-recovery coverage are not done.
  - Release smoke tests that import IDML, compose, and export PDF, EPUB and IDML.

## CLI direction

Exact command names are provisional. They must reuse the shared transaction,
planner and structured-error conventions.

```sh
# Structure.
pentool page add mag.pen --after 4 --count 2 --parent A-body
pentool parent create mag.pen B-feature --based-on A-body --columns 3
pentool section add mag.pen --at 9 --start 1 --style lower-roman

# Stories and frames.
pentool text place mag.pen feature.md --story feature-body --frame body-1 --autoflow --parent B-feature
pentool text thread mag.pen body-2 --after body-1
pentool text overset mag.pen --json

# Styles.
pentool style paragraph set mag.pen body --font "Source Serif 4" --size 10 --leading 13 \
  --align justify --hyphenate --composer paragraph --grid all-lines
pentool style grep add mag.pen body --pattern '\d+%' --char-style figures-bold

# Long documents and books.
pentool toc generate mag.pen --from heading-1,heading-2 --frame toc
pentool index generate guide.penbook --frame index
pentool book export guide.penbook guide.pdf --pdfx 4 --profile coated-fogra39

# Interchange and publishing.
pentool import mag.idml mag.pen --report fidelity.json
pentool export mag.pen mag.idml
pentool export mag.pen mag.epub --layout reflowable
pentool preflight mag.pen --profile print --json
```

## Acceptance targets

- The same document composes to identical line breaks and glyph positions, and
  exports byte-identical PDF, EPUB and IDML, on Linux, Windows, Intel macOS and
  Apple Silicon.
- The IDML corpus imports without crashing or losing content silently; every
  unsupported item appears in the fidelity report. For corpus files whose fonts
  are available, the published share of paragraphs matches InDesign's line
  breaks.
- An IDML file → `.pen` → IDML round-trip opens in InDesign, with stories,
  threads, styles and parents intact.
- A 300-page threaded book stays responsive in the editor while typing (the
  latency target is in `docs/performance.md`). A full recompose and PDF export
  of 500 pages finishes within the published benchmark.
- Exported PDFs pass veraPDF for PDF/X-4 and PDF/UA where tagged; EPUBs pass
  EPUBCheck.
- Changing a paragraph style reflows exactly the affected stories, and undo
  restores the original bytes.
- Hostile IDML and EPUB inputs fail safely without unbounded memory, path
  escape, or network access.
- Documents from earlier versions render exactly as they did before.

## Explicitly deferred

- Opening or writing native `.indd` and `.indb` binaries, which are
  undocumented. Users export IDML from InDesign. Revisit only with a documented,
  legally clear specification.
- Reproducing Adobe's interface, icons, product names, presets, sample content,
  or the Adobe Paragraph Composer's exact proprietary output. Pentool documents
  its own composer and measures agreement; it does not claim identical line
  breaks for every input.
- Adobe-specific services such as Creative Cloud Libraries, Adobe Fonts
  activation, Publish Online, Adobe Stock, and cloud review.
- Scripting (ExtendScript, UXP) and plugins, and executable content in
  documents. Automation goes through the CLI and agent API.
- Interactive and animated exports such as SWF and animated or HTML5
  publishing. Time-based media (animation, video, audio) is out of scope for Pentool.
- Vertical CJK layout (tategaki), ruby, warichu and the full Japanese
  composition rules (JIS X 4051). These need their own contract and may follow
  in a later release.

## Acceptance demo

Import a 24-page magazine exported from InDesign as IDML. It has parent pages, a
three-column feature with threaded frames, text wrap around a cut-out photo,
anchored pull quotes, a table that spans pages, and footnotes. Review the
fidelity report. In the editor, change the body paragraph style's leading and
watch exactly the affected stories reflow to the baseline grid. Thread a new
frame, fix the reported overset text, and regenerate the TOC. Add the issue to a
`.penbook` with two other issues, and build a book-wide index. Preflight it,
export PDF/X-4 that passes veraPDF and a reflowable EPUB that passes EPUBCheck,
then export IDML and open it in InDesign with its stories, styles and threads
intact. Show byte-identical exports on all four release targets.
