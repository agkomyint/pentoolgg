# v0.12.0 deferred work

v0.12.0 shipped on 2026-10-11 as the print-production foundation (see
[`docs/release-v0.12.0.md`](../../release-v0.12.0.md)). The owner scoped the
release down; the items below were not completed and are not claimed. They move
to a later print-completion milestone, to be numbered at planning time under the
versioning policy. The task text in [`TASKS.md`](TASKS.md) remains the
requirement for each.

| Item | TASKS.md | Needed to close |
|---|---|---|
| Vector and embedded-font PDF/X-4 output, spot inks in PDF | 11 | font embedding and licensing policy, vector writer, tests |
| Named independent PDF/X validator result | 11, 16 | run an external validator, record its verdict |
| Reference-RIP evidence | 9, 16 | plate/proof comparison against a reference RIP |
| Float scene and plate performance | 9, 16 | benchmark and optimization pass in `docs/performance.md` |
| Spot-to-process substitution, per-image resampling | 14 | recipe fields, planner, tests |
| Live canvas guides, browser interaction test | 15 | live guide updates while editing, browser harness |

Top-level boxes in `TASKS.md` are intentionally not ticked for these tasks.
