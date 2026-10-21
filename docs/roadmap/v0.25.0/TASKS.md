# v0.25.0 — Generative AI in Pentool Studio

> Status: planned. Nothing below is implemented; boxes are ticked only when
> implementation, tests, docs and cross-platform evidence exist.

0.25.0 ships the accumulated 0.24.x internal patches (see `docs/release-v0.24.2.md`)
together with the first generative-AI workflow in Studio. The engine already exists
as `pentool ai` (`src/ai.rs`, `docs/ai.md`, `docs/roadmap/v0.9.0/TASKS.md`); this
release gives it a browser UI. It completes the "editor UI" item deferred by v0.9.0.

## Research summary

- Photoshop Generative Fill: select an area; a floating bar offers a prompt box, a
  Generate button and a model picker; Firefly returns several variations, partner
  models one; the result lands in a separate generative layer (non-destructive);
  Generative Expand lives in the Crop tool; Content Credentials are attached on
  export; credits are shown per generation.
- Canva Magic Studio: one-click tools (Magic Edit, Eraser, Expand, Write) reached
  from the editor toolbar/Apps menu, several variations to choose from, credit-based.
- Shared pattern: *act on a target, get a few candidates, preview, accept into a
  separate layer.* Pentool's run/candidate/accept model already matches this.

## Product position

AI sits beside crop, mask and fill, not behind a chat agent. Every model call is an
explicit, confirmed user action that produces candidates; the document changes only
on Accept, as one undoable transaction. Opening, rendering and exporting never call
a model.

## Non-negotiable boundaries

- Offline by default: with no provider configured, Studio never touches the network;
  `PENTOOL_AI=off` hides the whole AI UI.
- Keys are references (`credential_env` / `credential_file`). The browser never
  receives, stores or sends a key; the server never echoes one.
- No model call without an explicit confirmation naming provider, model, candidate
  count and the dry-run summary (prompt hash, host, source hashes).
- Reuse `ai::run` paths (limits, endpoint/host policy, provenance, stale-source
  checks); no parallel implementation in `server.rs`.
- Every new route is classified in `perm::ROUTES`: provider settings need `owner`,
  generate/accept need `editor`, run listing needs `viewer`.
- Studio source lives in `web-ui/`; `web/studio.html` is rebuilt by the integrator.
- No new dependencies; no `.pen` field changes (the existing `ai` provenance object
  is reused).

## Tasks

### 1. Server API
- [ ] `GET /api/ai/status`: enabled flag, configured providers (names, adapters,
      capabilities, default model, key *source kind* only), limits and
      `limits.problems`.
- [ ] `POST /api/ai/connect` and `/api/ai/doctor`: wrap `connect`/`doctor` with the
      CLI's validation and error codes.
- [ ] `POST /api/ai/generate`: `dry_run` returns the CLI dry-run report;
      a live call requires `allow_model_call: true`; returns the run id and
      candidate thumbnails (bounded size).
- [ ] `GET /api/ai/runs`, `GET /api/ai/runs/:id/candidates/:n`, `POST /api/ai/runs/:id/accept`
      (new node or `replace`, `if_revision`, `dry_run`) and `.../discard`.
- [ ] Model calls run off the async executor, honour `PENTOOL_AI_TIMEOUT_SECS`, and
      can be cancelled by the client.
- [ ] Failed calls leave the document byte-identical; errors keep the CLI `code` and
      `fix` fields.

### 2. Studio: provider settings
- [ ] "Connect a provider" panel: adapter, endpoint, model, credential environment
      variable *name* or key file path; Test runs `doctor`. Clear states for
      disabled, unconfigured, unreachable and quota errors.

### 3. Studio: Generate panel
- [ ] Prompt, aspect/size, candidate count (capped by the server limit).
- [ ] Confirmation step showing the dry-run summary before any billed call.
- [ ] Candidate grid with Accept as new layer, Replace selected image, Discard;
      progress and Cancel while waiting.
- [ ] Accepted result shows a provenance badge (provider, model; never the prompt).
- [ ] Undo removes an accepted result in one step; a Studio reload keeps run history
      (bounded to the existing 16 runs per document).

### 4. Studio: selection actions on existing images
- [ ] Remove background (existing `edit --kind remove-background` plus local key-out).
- [ ] Create variations of the selected image.
- [ ] Masked inpaint/outpaint (Replace Area, Remove Object, Extend Canvas) are
      **deferred** until the engine supports masks; see Explicitly deferred.

### 5. Release
- [ ] CLI `--help`, README and `docs/ai.md` updated; Studio docs updated.
- [ ] Release notes merge `docs/release-v0.24.2.md` and this work.
- [ ] Version bump to 0.25.0 in `Cargo.toml` and `Cargo.lock`, CI green on all three
      OSes, then tag and publish (only when the maintainer says so).

## Acceptance tests

- Route scan: every `/api/ai/*` route is in `perm::ROUTES`
  (`every_route_is_classified`).
- A mock provider (loopback HTTP server in the test) drives generate → candidates →
  accept → undo, and replace with a stale source, with byte-exact document checks.
- No request reaches the mock when the confirmation flag is absent or `PENTOOL_AI=off`.
- No response body, log line or document contains the key value (planted sentinel).
- Viewer role cannot connect, generate or accept; editor cannot change providers.
- Dry run and live call report compatible results.
- Live-browser check of the Generate panel on Windows, Linux and macOS.

## Explicitly deferred

- Masked inpaint/outpaint, Remove Object, Extend Canvas, upscale, recipe/regenerate.
- Cost estimate or spend budget (`PENTOOL_AI_MAX_COST`); only call counts are shown.
- Chat-style agent inside Studio, OS keyring, hosted gateway, Pentool-run cloud.
- Content Credentials / C2PA signing (provenance stays the `ai` object).
- Additional adapters beyond `gemini` and `openai-compatible`.
- Ed25519 collaboration signing and stale-base rejection (separate roadmap debt).

## Open decisions for the maintainer

1. Scope: masks in 0.25.0 or later (this plan assumes later).
2. Cost display: call count only (assumed) or an estimate per provider.
3. Provider list: the two existing adapters (assumed) or more.
