# v0.14.0 remaining work (handoff)

Work on v0.14.0 is paused. This file lists what is done and what is left so
any human or agent can resume. `TASKS.md` stays the contract; checkboxes there
are deliberately unticked until implementation, tests, docs and cross-platform
CI evidence all exist.

Last commits: `9fcca3f` (task 6 style core), `f8fdf70` (task 5 composer).
Nothing was pushed.

## Implemented (committed locally, boxes unticked)

| Task | Module / command | Contract |
| --- | --- | --- |
| 1 Spec, IDML mapping, threat model | `docs/layout-v1.md`, schemas, fixtures | complete |
| 2 Spreads, parents, sections | `src/spreads.rs`, `spread`/`parent`/`section`/`override` | core done |
| 3 Stories, threaded frames | `src/stories.rs`, `story`/`frame` | core done; uses a width estimator |
| 4 Shaping and fonts | `src/shaping.rs`, `text shape` | core done |
| 5 Paragraph composer | `src/compose.rs`, `text compose` | core done |
| 6 Styles | `src/styles.rs`, `layout-style` | core done (see below) |

## Open items inside started tasks

- **Task 5:** tab stops with leaders; span/split columns; last-line indent edge
  cases; line-break goldens across the IDML corpus; wire the composer into
  frames (replace the task 3 estimator) and render composed frames; PDF
  subset-font embedding; cross-platform CI evidence.
- **Task 6:** nested, GREP and list evaluation are done (`src/style_eval.rs`).
  Open: composed-line evaluation of line styles in frames; `after-section` list
  restarts; IDML style import; shortcuts; style changes recompose exactly the
  affected stories via the usage index.
- **Tasks 2-4:** see the Progress notes in `TASKS.md`.

## Not started

7 grids/guides/alignment; 8 text wrap and anchored objects; 9 tables;
10 long-document features; 11 `.penbook` books; 12 data merge and adaptive
layout; 13 IDML import; 14 IDML export; 15 publishing exports; 16 layout
preflight; 17 editor layout experience; 18 conformance, performance, recovery.

## Resume checklist

1. `git log --oneline` and read the Progress notes in `TASKS.md`.
2. Build with `CARGO_TARGET_DIR=<dir> cargo test --profile fast --all-targets --locked`
   (tests: `tests/v0140_*.rs`).
3. Continue with the style open items, then task 7.
4. Never push without the owner's say-so.
