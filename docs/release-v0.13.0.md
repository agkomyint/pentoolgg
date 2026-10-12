# Pentool v0.13.0

Agent-directed realistic painting. The `.pen` format is unchanged by the
painting work; existing documents, CLI flags and JSON fields keep their behavior.
The protocol is frozen in `docs/agent-painting-protocol-v1.md`.

## Painting

- Semantic stroke paths, deterministic stroke fields and region-aware
  constraints, so an agent describes intent rather than pixels.
- Color sampling, mixing and portable palettes (`paint-palette`).
- Material-aware media (`paint-media`) and reusable composition and form guides.
- Declarative multi-pass paint plans (`paint-plan`) with dry run, revision and
  plan-hash guards, per-pass history and cooperative cancellation. A cancelled
  plan names its last completed stage and writes nothing.
- Compact visual analysis, region previews, reference comparison with a
  value-difference heat map, and revision scoring against a goal.
- Paint bundles, templates, diff and package integration.

## Studio

- Plan authoring and review: palette extraction and point sampling, reference
  comparison with a difference overlay, crop preview, field-vector overlay and a
  per-pass material list. Endpoints are `/api/paint/{plan,score,palette,compare,preview,field,media}`.

## Reliability

- A document left missing by a writer killed between the two renames of the
  atomic write is restored from its newest valid `.NAME.pentool-old-*` copy the
  next time it is opened or committed.
- Conformance, fuzz and bounded-session tests, including a 10,048-stroke run
  (`cargo test --release --test v0130_conformance -- --ignored`).

## Also in this release

- Early v0.14.0 layout groundwork (spreads, parents, sections, stories, threaded
  frames, the shaping engine, the paragraph composer and layout styles) is
  present in the source but the v0.14.0 roadmap is not complete; treat it as
  unreleased.
