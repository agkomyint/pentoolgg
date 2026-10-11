# Architecture milestone — one core, API-first server, live canvas

Status: proposed. This milestone is cross-cutting: it does not add a
user-visible feature set, it makes every later roadmap item cheaper, faster and
safer. It may ship as several patch/minor releases chosen at release time.

Focus: the best tool for artists and creators working in static 2D (vector,
images, painting, layout) plus static 3D for packaging and mockups. Time-based
media is out of scope (see `AGENTS.md`).

## Product contract

- **One Rust core.** Document model, validation, scene operations, history,
  geometry, color and the deterministic CPU renderer live in a library crate
  that compiles natively and to WebAssembly. The CLI, server and browser share
  the same code, so a result never depends on which surface produced it.
- **`pentool serve` is authoritative.** It owns files, history, export and the
  API. `pentool serve --no-ui` is the headless mode for agents, CI and hosting.
  Studio is a thin client of the same versioned API.
- **The CPU renderer stays normative.** Exports, goldens and digests come only
  from it. The browser may draw a live view with WASM and, later, WebGPU, but
  the live view is a preview of the same document and is never the reference.
- **Offline and single binary.** The WASM module and Studio are embedded in the
  binary; no Node.js, network or database is required at run time.

## API contract

- Versioned `/api/v1` with an OpenAPI description generated from the same Rust
  types; existing `/api/*` paths keep working until a documented removal.
- Every mutation carries an actor (`human`, `agent`, `cli`) and a revision guard;
  results report the new revision and stable object IDs.
- Event stream (server-sent events) for document, history and job changes, so
  several clients and agents observe one document without polling.
- Authentication is required whenever the server binds a non-loopback address.
  Loopback stays zero-configuration.
- Errors are structured: operation, page, object, cause and corrective action.

## Must ship, in order

- [ ] **1. WASM spike.** Compile the document model and the CPU renderer to
  `wasm32-unknown-unknown`; record binary size, cold start, and the list of
  dependencies that do not compile. Decision record in `docs/architecture.md`.
  Nothing else starts until this is measured.
- [ ] **2. Core crate split.** Extract the platform-independent core
  (`pentool-core`) from the binary crate behind the existing public behavior;
  file, process and network access stay in the binary. Existing tests pass
  unchanged.
- [ ] **3. `/api/v1` and `--no-ui`.** Stable routes, OpenAPI, actor identity,
  event stream, loopback/auth policy, and a parity test that drives the same
  edit script through the CLI and the API and compares digests.
- [ ] **4. Live canvas.** Studio renders the document through the WASM core on a
  canvas with exact hit-testing (shape geometry, not bounding boxes), local
  incremental redraw while dragging, and server round trip only on commit.
- [ ] **5. Live brush.** Painting strokes draw locally for immediate feedback
  from the same brush engine, then commit as one deterministic request; the
  committed pixels equal a replay of the stroke on the server.
- [ ] **6. Task workspaces.** Studio organizes by task (Draw, Paint, Photo,
  Compose, Layout, Print, and Package once v0.18.0 ships) over one document and
  one layer stack, without separate editors per area.
- [ ] **7. Optional `gpu` feature.** WebGPU/`wgpu` acceleration for live
  filters and the 3D viewport, behind the feature flag, with a CPU fallback and
  a tolerance test against the CPU reference.
- [ ] **8. Budgets and evidence.** Per-feature binary-size and frame-time
  budgets enforced in CI (`docs/performance.md`), cross-platform parity goldens,
  and hostile-input cases for every new API surface.

## Acceptance targets

- The same edit script through CLI, API and Studio produces byte-identical
  `.pen` output and equal render digests on Linux, Windows and Apple Silicon.
- `pentool serve --no-ui` starts without the Studio assets being reachable and
  every Studio operation is available through `/api/v1`.
- A 1,000-object page stays interactive: drag redraw within the frame budget
  recorded in `docs/performance.md`.
- A brush stroke shows immediately in the browser and the committed document
  matches the server-side replay pixel for pixel.
- Binary size growth for this milestone is within the budget set by task 1.

## Explicitly deferred

- Real-time multi-user editing and CRDT merge (v0.24.0 collaboration).
- Hosted multi-tenant operation (v0.25.0).
- Replacing the browser with a native GUI toolkit. The browser remains the
  front end because it ships everywhere with no per-platform UI code.
- Video, audio and timeline features.
