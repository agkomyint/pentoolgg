# v0.13.0 — Agent-directed realistic painting

Make the deterministic v0.10 raster engine practical for software agents that
need to paint natural-looking images. Agents should express artistic intent—form,
region, direction, value, material, palette, and revision goals—while Pentool
produces bounded canonical samples and ordinary editable raster operations.

This is a minor release because it adds commands, scene data, analysis output,
and painting capabilities. Patch releases in the v0.10 line remain limited to
bug fixes and documentation.

## Product contract

- The Rust raster engine remains authoritative. Browser, CLI, API, and batch use
  the same planners, sampling algorithms, transactions, and validation.
- Every committed result is deterministic from its engine version, normalized
  inputs, materialized dependencies, selections, palettes, fields, and seeds.
- High-level commands compile to inspectable canonical operations. They never
  hide a remote model call, execute scripts, or mutate unrelated regions.
- Agents work through bounded render-inspect-refine loops rather than receiving
  unrestricted full-resolution pixel dumps.
- Semantic constraints such as `inside`, `avoid`, contour following, and sampled
  palettes are pinned by content hash so replay cannot reinterpret later edits.
- Reference images remain immutable sources. Comparison and analysis never alter
  a document unless a separate explicit mutation is committed.
- Dry run and commit use one planner. Failed, cancelled, stale, over-budget, or
  locked operations leave the document byte-for-byte unchanged.
- Existing v1-v6 documents render identically unless a user applies a new v0.13
  operation. New metadata must remain readable JSON and safely ignorable by older
  readers wherever the format contract permits.
- Offline documents remain complete. No AI provider, network service, database,
  GPU, or Node.js runtime is required to reopen, replay, inspect, or export them.

## Agent painting model

The basic agent loop is:

```text
inspect -> plan bounded regions -> dry-run one pass -> commit -> render crop
        -> compare structure/color/value -> revise only weak regions -> repeat
```

Pentool owns conversion from compact artistic instructions to canonical dabs.
The agent owns the explicit region, brush, palette, intent, budget, and decision
to accept another pass. A plan may contain several ordered passes, but one plan
commits as one history entry and must be reviewable before execution.

## Must ship, in order

- [x] **1. Freeze the agent-painting protocol and threat model.** Specify
  high-level paths, pressure curves, region constraints, sampled colors, palettes,
  vector fields, paint plans, comparison metrics, canonical compilation, hashes,
  engine compatibility, cancellation, resource limits, and stable JSON results.
  Publish schemas plus valid, invalid, adversarial, and cross-platform fixtures
  before adding mutation commands. Explicitly bound path segments, samples,
  generated dabs, fields, regions, palette size, passes, pixels, and output.

- [x] **2. Add deterministic semantic stroke paths.** Accept SVG-compatible line,
  quadratic, cubic, and arc paths instead of requiring agents to manufacture
  dense pointer events. Add piecewise size, pressure, flow, opacity, rotation,
  and color curves along normalized path distance. Define flatness, arc-length
  approximation, corner behavior, endpoint inclusion, interpolation, and rounding.
  Compilation emits the existing canonical raster samples and reports both the
  compact source and generated sample/dab counts.

- [x] **3. Add color sampling, mixing, and portable palettes.** Sample a point,
  bounded region, selection, layer, or composite through the authoritative
  renderer. Extract deterministic dominant, median, highlight, midtone, shadow,
  and edge palettes. Add explicit color interpolation/mixing and bounded seeded
  variation per stroke or dab. Store named palettes as readable document data;
  preserve source scope, color space, algorithm version, and source hash.

- [x] **4. Add fast region previews and structural comparison.** Export a crop,
  node, layer, selection, or changed bounds with `max-edge` and pixel budgets.
  Compare a rendered region with an immutable reference using documented color,
  value, edge, silhouette, structure, and texture-frequency metrics. Results are
  compact JSON with heat-map artifacts available only when requested. Metrics are
  guidance, not a claim of perceptual equivalence or artistic quality.

- [x] **5. Add region-aware painting constraints.** Allow painting inside or
  outside active/saved selections, avoiding protected selections, clipping to
  opaque pixels, and stopping at bounded color, alpha, luminance, or detected-edge
  barriers. Add `follow contour`, `follow gradient`, and explicit direction modes.
  The exact resolved constraint mask/field is previewable, hashed, journaled, and
  evaluated consistently across tiles without seams or boundary double-application.

- [x] **6. Add deterministic stroke fields.** Generate bounded direction fields
  from contours, gradients, distance transforms, structure tensors, perspective
  guides, or supplied vectors. Paint seeded strokes through a field with explicit
  density, length, curvature, overlap, jitter, taper, and collision rules. Fields
  are inspectable, cacheable derived data; committed strokes pin the exact field
  digest. This enables hair, fur, grass, fabric, wood, and form-following hatching.

- [x] **7. Add lighting- and material-aware paint modes.** Define deterministic
  glaze, dry-brush, bristle, impasto-like, palette-knife, watercolor-wash,
  charcoal, pencil, canvas-grain, and paper-grain behavior without pretending to
  simulate unbounded physical media. Support explicit light direction, shadow /
  midtone / highlight targeting, preserve-luminance and preserve-chroma controls,
  pickup/deposit mixing, wetness lifetime, texture strength, and edge behavior.
  Algorithms and texture assets are versioned, bounded, content-addressed, and
  replayable; marketing names must not obscure the documented pixel math.

- [x] **8. Add compact visual analysis for agents.** Report major contours,
  silhouettes, value zones, dominant colors, focal candidates, gradient direction,
  texture frequency, coverage gaps, clipping, and suggested stroke orientation for
  an explicit bounded scope. Return polylines, regions, histograms, and summaries,
  never an unbounded pixel dump. Analysis is read-only, paginated, deterministic,
  and tagged with renderer, algorithm, page, scope, revision, and source hashes.

- [x] **9. Add declarative multi-pass paint plans.** Define a data-only plan with
  ordered passes such as underpainting, local color, form shadows, highlights,
  edge work, texture, and cleanup. Each pass names its scope, palette, brush,
  constraints, path or field source, blend, opacity, seed, and budget. Planning
  resolves every dependency, estimates generated samples/dabs/tiles, previews
  changed bounds, and makes no mutation. Commit is atomic, revision-guarded, and
  one undo entry. Partial success is forbidden.

- [x] **10. Add reusable composition and form guides.** Provide non-exporting
  thirds, golden-ratio, symmetry, horizon, perspective, vanishing-point, gesture,
  face/body proportion, value-map, and custom guide primitives. Snapping and
  stroke fields may reference guides by stable ID. Guides remain ordinary safe
  document data, can be hidden or explicitly included in proof exports, and never
  silently change pixels when edited.

- [x] **11. Add agent-focused diagnostics and revision scoring.** Mutation
  results report changed bounds, coverage, value range, palette delta, edge spill,
  protected-region violations, stroke/dab/tile counts, comparison deltas, and
  actionable warnings. Diagnostics distinguish measured facts from heuristic
  suggestions. Provide before/after scoring for a declared goal without silently
  accepting, reverting, or continuing a plan.

- [x] **12. Integrate presets, batch, history, diff, packages, and components.**
  Portable presets can bundle brushes, palettes, fields, guides, and plan templates
  but no code. Structural diff summarizes plan/pass/constraint changes and tile
  impact. Packages collect all accepted assets and hashes. Component instances and
  masks preserve existing override semantics. History, undo/redo, dry run, locks,
  page selection, revision guards, and unknown fields behave like other mutations.

- [x] **13. Build editor-native review and authoring controls.** Add path/pressure
  editors, palette sampling, protected regions, field visualization, material
  controls, pass ordering, crop previews, reference comparison, metric overlays,
  and exact disclosure of generated work. Preview may be approximate; commit must
  replace it with authoritative Rust output. Agent-produced plans appear as normal
  editable operations, not an autonomous chat surface.

- [ ] **14. Harden conformance, performance, and failure recovery.** Add golden
  tests for path compilation, palette extraction, fields, constrained tile edges,
  materials, plans, metrics, and exports on Linux, Windows, Intel macOS, and Apple
  Silicon. Fuzz schemas, curves, fields, plans, references, palettes, and hostile
  dimensions. Test cancellation at every stage, stale revisions, damaged assets,
  locks, memory/disk budgets, transaction rollback, crash recovery, packaging,
  offline replay, and 10,000-pass/stroke bounded-session behavior.

  Status (left unchecked on purpose): implemented and passing locally on Windows
  are plan, tile and canonicalization goldens, fuzzing of plans, palette colors,
  reference images and field specs, hostile dimensions, locks, stale revisions,
  rollback, cancellation at every plan checkpoint, automatic recovery of a write
  killed between `atomic_write`'s renames, leftover temp files, history restore,
  plan/dab/pixel/tile and staged-resource byte budgets, packaging, and the
  10,048-stroke run (`cargo test --release --test v0130_conformance -- --ignored`,
  35 s). The only open evidence is cross-platform: the digests in
  `tests/v0130_conformance.rs` run in the existing CI matrix (Linux, Windows,
  Apple Silicon) on the next push, and AGENTS.md requires that green run before
  this box is ticked. Pushing is the maintainer's call, so no CI run exists yet.

## CLI direction

Exact command names are provisional. They must reuse the shared raster planner and
stable structured-error conventions rather than becoming independent code paths.

```sh
# A compact curve becomes deterministic canonical samples and dabs.
pentool raster portrait.pen stroke-path paint \
  --path 'M220 840 C260 610 390 420 570 260' \
  --brush oil-round --size-curve '0:18,0.45:52,1:24' \
  --pressure-curve '0:0.2,0.5:0.8,1:0.5' --color '#8B5A3C' --seed 42

# Sample and reuse colors rather than guessing literals.
pentool paint sample portrait.pen --scope composite --x 420 --y 330 --json
pentool paint palette extract portrait.pen skin \
  --selection face --colors 8 --roles shadows,midtones,highlights
pentool paint color mix '#A76C54' '#E7B59A' --amount 0.35 --json

# Resolve and preview constraints before painting.
pentool paint constraints portrait.pen preview \
  --inside selection:face --avoid selection:eyes \
  --follow contour --edge-threshold 0.18 --output constraints.png

# Generate and inspect a direction field, then paint through it.
pentool paint field create portrait.pen hair-flow \
  --source composite --selection hair --method structure-tensor --scale 12
pentool paint field inspect portrait.pen hair-flow --max-vectors 200 --json
pentool paint field stroke portrait.pen paint --field hair-flow \
  --brush bristle --palette hair-palette --density 0.7 \
  --length 12..60 --taper 0.8 --seed 43 --dry-run

# Paint explicit value zones with material-aware behavior.
pentool paint stroke portrait.pen paint --selection lit-planes \
  --brush dry-bristle --blend glaze --tone highlights \
  --lighting-direction -35 --preserve-luminance 0.4 --texture-strength 0.25

# Inspect only the bounded context an agent needs.
pentool analyze portrait.pen --selection face \
  --contours --values 5 --palette 8 --texture --stroke-orientation --json
pentool export portrait.pen face-preview.png \
  --rect 300 200 400 400 --max-edge 512
pentool compare portrait.pen --reference reference.png \
  --rect 300 200 400 400 --metrics color,value,edges,structure --json

# Validate, estimate, preview, and atomically commit an ordered plan.
pentool paint plan portrait.pen portrait-plan.json --dry-run --json
pentool paint plan portrait.pen portrait-plan.json \
  --if-revision 0123456789abcdef --max-dabs 250000 --json

# Composition aids remain non-exporting unless explicitly requested.
pentool guide add portrait.pen thirds --kind thirds
pentool guide add portrait.pen perspective \
  --kind perspective --vanishing-point 900 340
pentool guide add portrait.pen value-map --kind value-map --levels 5
```

## Paint-plan direction

A plan is declarative, data-only, and independently validatable:

```json
{
  "schema": 1,
  "id": "portrait-finish",
  "page": "page-1",
  "target": "paint",
  "passes": [
    {
      "id": "underpainting",
      "selection": "subject",
      "brush": "large-soft",
      "palette": "portrait-base",
      "field": "form-contours",
      "blend": "normal",
      "opacity": 0.85,
      "seed": 41
    },
    {
      "id": "form-shadows",
      "selection": "subject-shadows",
      "avoid": ["eyes", "highlights"],
      "brush": "soft-glaze",
      "palette": "portrait-shadow",
      "field": "form-contours",
      "blend": "multiply",
      "opacity": 0.22,
      "seed": 42
    },
    {
      "id": "highlights",
      "selection": "lit-planes",
      "brush": "dry-bristle",
      "palette": "portrait-light",
      "blend": "screen",
      "opacity": 0.18,
      "seed": 43
    }
  ],
  "limits": {
    "max_passes": 16,
    "max_generated_strokes": 5000,
    "max_dabs": 250000,
    "max_changed_pixels": 16000000
  }
}
```

The dry-run response includes resolved revisions and hashes, changed bounds per
pass, generated stroke/sample/dab estimates, tile and memory budgets, warnings,
and the exact command required to commit with a revision guard.

## Structured diagnostic direction

Painting commands return compact evidence suitable for another agent iteration:

```json
{
  "changed_bounds": [210, 180, 680, 920],
  "coverage": 0.42,
  "value_range": [0.13, 0.84],
  "palette_delta": 0.07,
  "edge_spill_pixels": 0,
  "protected_region_violations": 0,
  "generated_strokes": 138,
  "dabs": 18420,
  "tiles_changed": 23,
  "warnings": [
    {
      "code": "low-highlight-coverage",
      "message": "Highlights cover less than 2% of the declared subject."
    }
  ]
}
```

All ratios define their denominator and scope. Empty regions and unavailable
metrics return explicit states rather than invented zeroes.

## Acceptance targets

- Reproduce one semantic Bézier stroke byte-for-byte across all release targets,
  including its compiled samples, dabs, tile hashes, journal, and exported pixels.
- Paint hair or fur through a pinned contour/structure field without seams,
  protected-region spill, or changes outside the declared selection.
- Extract a palette from a reference scope, move the project offline, and replay
  the accepted painting without the original external file or network access.
- Complete a three-pass underpaint/shadow/highlight plan as one atomic history
  entry; dry run and commit report compatible work, and undo restores exact bytes.
- Render and compare bounded crops repeatedly without changing the document or
  emitting unbounded data; identical inputs produce identical metrics and maps.
- Cancel path compilation, field generation, material painting, comparison, or a
  multi-pass plan at every supported boundary without partial mutation or orphan
  tiles/assets.
- Package brushes, palettes, fields, guides, accepted plans, raster data, and all
  content-addressed dependencies; verify and reproduce the render offline.
- Demonstrate a complete agent loop in which only explicitly weak regions are
  revised and every iteration stays within declared pass, dab, pixel, and time
  budgets.

## Explicitly deferred

- Autonomous whole-document redesign, unsupervised endless refinement loops, and
  automatic acceptance or rollback based on a heuristic score.
- A claim that comparison metrics measure beauty, identity, style fidelity, or
  human preference. They measure only their documented pixel/structure properties.
- Remote image generation, semantic segmentation, depth estimation, or vision
  models hidden behind core painting commands. Optional AI outputs must first be
  materialized through the v0.9 review/provenance workflow.
- Unbounded fluid, pigment, cloth, hair, or physics simulation; executable brush
  scripts; native plugins inside Pentool; and nondeterministic GPU-only results.
- Copyrighted artist-name imitation presets or built-in prompts. Materials and
  presets describe observable mechanics rather than living-artist identity.
- PSD fidelity guarantees, animation/video painting, 3D scene painting, and
  sculpting. These require separate format and product contracts.

## Acceptance demo

Open a layered portrait with an immutable reference. Analyze its bounded face and
hair regions, extract role-based palettes, and create protected eye/highlight
selections. Build a form-contour field for glazing and a structure field for hair.
Dry-run and commit an underpaint, shadow, and highlight plan; then paint seeded
hair strokes through the pinned field. Render a 512-pixel face crop, compare its
value, edge, color, and structure metrics with the reference, and revise one weak
region only. Inspect the canonical strokes, hashes, changed bounds, diagnostics,
and one history entry. Finally package the document, clear disposable caches,
move it offline, replay verification, and reproduce identical exports on every
supported release target.
