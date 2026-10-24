# v0.28.0: Selection you can see and paint

Status: in progress

Goal: Studio selections show their real shape and can be painted with a brush, so
a person (and an agent driving Studio) can select a subject the way they would in
a photo editor. v0.27.0 shipped the engine; its Studio outline was only the
selection's bounding box.

Invariants: no new dependency, no model, no network; `.pen` files unchanged;
existing flags and JSON fields keep working (new fields are additive).

## Tasks

### 1. Real selection shape in Studio
- [x] Every selection result carries a bounded `preview` coverage grid (scale,
      cols, rows, base64 data; at most 256 cells on the long edge).
- [x] Studio tints the selection and draws its true outline, not the box.
- [x] Unit test for the preview grid.

### 2. Selection brush
- [x] A "Selection brush" tool (A) paints into the selection with the shared
      quickmask engine action; Alt or subtract mode removes.
- [ ] Live tinted stroke preview matches the committed result (preview exists;
      needs a live-browser check).

### 3. Finish the v0.27.0 gaps
- [ ] `cutout` mask source and `--preview`.
- [ ] Mask from selection (non-destructive).
- [ ] Move, transform and keyboard-nudge the selection outline in Studio.
- [ ] Select on an image node without a manual rasterize step (Studio).

### Explicitly deferred
Operation signing, stale-base rejection, learned subject detection, new
dependencies, time-based media.

### Reports (maintainer-approved addition)

Scope added by the maintainer on 2026-10-25: reports from multiple data sources
with citations and visualization. Invariants: no new dependency; reuse
`merge::parse_records`; deterministic output; data paths stay inside the spec
directory; nothing is written when a build fails.

- [x] `pentool report build|check|data|example` (`src/report.rs`).
- [x] Sources with kind/publisher/url/date, numbered citations, Sources page.
- [x] Blocks: heading, text, callout, stats, bar, column, segmented, table.
- [x] Auto layout with continuation pages; `--strict` warnings.
- [x] Tests in `tests/report.rs`; docs in `docs/reports.md`.
- [ ] Studio panel for reports (explicitly deferred: CLI first).
