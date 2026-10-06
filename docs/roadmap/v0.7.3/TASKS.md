# v0.7.3 — BYOK image models as first-class editing tools

Integrate user-chosen image models into Pentool's existing image workflow without
turning the editor into an autonomous agent or coupling `.pen` documents to one AI
vendor. Users bring their own provider account, API key, gateway, or local model.
Pentool exposes generation and editing as explicit, reviewable image operations,
materializes every accepted result as a verified asset, and keeps the document
renderable offline afterward.

The design borrows the useful boundary from provider-neutral SDKs: a stable core
request and result contract, a provider/model registry, capability discovery, and
an escape hatch for namespaced provider options. Pentool does not embed a
JavaScript runtime or require an AI SDK, Node.js, hosted gateway, or Pentool cloud
service. Adapters implement the contract directly or through an external tool
protocol while preserving the one-binary default distribution.

## Product position

AI belongs beside crop, mask, cleanup, fill, and adjustment controls—not behind a
chat agent that describes and manipulates the whole document. The editor should
offer focused actions such as Generate, Replace Area, Remove Object, Extend Canvas,
Remove Background, Restore, Upscale, and Create Variations. Each action receives
explicit scene inputs and produces candidates that the user can compare, accept,
or discard.

An accepted result becomes an ordinary content-addressed v0.7 image asset. The
corresponding recipe and run provenance may remain in the `.pen` document so the
result can be understood, revised, or explicitly regenerated. Rendering, opening,
validating, packaging, and exporting a document never invoke a model or require a
network connection.

## Non-negotiable boundaries

- BYOK means credentials remain in environment variables, OS credential storage,
  or explicitly configured local provider profiles. API keys, bearer tokens,
  signed URLs, account IDs, and secret headers MUST NOT be written to `.pen`,
  history, logs, packages, crash reports, or command output.
- Model execution is always an explicit user or CLI action. Opening, previewing,
  rendering, validating, diffing, installing, or exporting never reruns a recipe.
- Generated pixels are untrusted input and pass through the same signature,
  decoder, dimension, memory, metadata, and hash checks as imported images.
- A remote failure, cancellation, moderation refusal, timeout, malformed response,
  or exhausted budget leaves the document byte-for-byte unchanged.
- A `.pen` file stores materialized outputs, portable recipe intent, and sanitized
  provenance—not a promise that a retired or changed model can reproduce pixels.
- Prompts and reference images can be sensitive. Commands and editor flows MUST
  show exactly what will leave the machine before submission.
- No provider may receive the whole document implicitly. Only the selected,
  flattened inputs, masks, references, and declared metadata are disclosed.
- Provider terms, licenses, output restrictions, and safety policy remain visible;
  Pentool does not claim ownership or commercial rights for model output.

## Portable model contract

Pentool defines capability classes rather than a lowest-common-denominator API:

- `generate`: text and optional reference images to a new image;
- `edit`: instruction plus one or more source/reference images;
- `inpaint`: edit only a supplied mask region;
- `outpaint`: extend an image into an explicitly sized canvas;
- `remove-background`: produce transparency or a reusable mask;
- `remove-object`: remove masked content and reconstruct the area;
- `restore`: denoise, deblur, repair, or colorize with declared intent;
- `upscale`: enlarge with a requested scale or target size;
- `variation`: create alternatives while retaining source composition.

An adapter reports which capabilities, input counts, mask conventions, media
types, dimensions, aspect ratios, seeds, candidate counts, and provider-specific
options a selected model supports. Unsupported portable options fail before any
billable request. Silent parameter dropping is forbidden; adapter warnings are
returned and retained with the run.

Portable request fields include operation kind, prompt, negative prompt, input
asset hashes, reference roles, mask hash and polarity, requested size or aspect
ratio, candidate count, seed when supported, output media type, timeout, and
canonical namespaced provider options. Provider/model identifiers are strings,
not a hard-coded enum, because available models change independently of Pentool.

## `.pen` representation

Add document-level AI recipes and immutable run records rather than making remote
execution part of the renderer. A conceptual recipe is readable JSON:

```json
{
  "id": "cleanup-product-photo",
  "kind": "remove-background",
  "inputs": [{ "asset": "sha256:...", "role": "source" }],
  "prompt": "Preserve the product and its natural translucent edges.",
  "mask": null,
  "output": { "alpha": true, "size": "source" },
  "provider_options": {}
}
```

The provider profile and credential selector are local execution configuration,
not portable document state. A run records the recipe revision/hash, adapter and
protocol versions, provider and returned model identifiers, input and output
hashes, sanitized request settings, seed if honored, provider warnings, usage when
available, timestamps, reproducibility claim, and acceptance status. Raw provider
responses and headers are not embedded by default.

Prompt storage is explicit: `document` keeps editable prompt text, `private`
stores only a digest plus a local reference, and `redacted` retains a digest and
human-authored summary. A private or redacted recipe remains renderable from its
materialized output but cannot be regenerated elsewhere without the missing input.

AI recipes are not evaluated live in an image operation stack. Instead, accepted
results create or replace normal image assets through one transaction. The run
record links source, result, mask, and recipe so structural diff and history can
explain the change without making ordinary rendering nondeterministic.

## Must ship, in order

- [ ] **1. Freeze the AI image protocol and threat model.** Specify portable
  requests/results, capability negotiation, canonical JSON, input disclosure,
  credential boundaries, redaction, error taxonomy, cancellation, retries,
  timeouts, usage reporting, and output validation. Publish normative schemas and
  valid/invalid fixtures before enabling any network adapter.

- [ ] **2. Add provider profiles and a model registry.** Support named local
  profiles containing adapter kind, endpoint policy, credential source, and safe
  defaults. Discover or configure models with stable string IDs and cached
  capability snapshots. Listing models may contact a provider only through an
  explicit refresh; document open and editor startup use local state.

- [ ] **3. Implement a versioned adapter boundary.** Add an in-process HTTP
  adapter interface and a sandboxable external-process JSON protocol for local or
  community backends. Requests use local byte streams or content-addressed handles,
  never arbitrary document paths. Bound stdout/stderr, response bytes, redirects,
  hosts, execution time, and child-process authority.

- [ ] **4. Ship a small reference adapter set.** Choose adapters from maintained
  providers only after conformance tests exist. Include one OpenAI-compatible
  remote path, one provider that supports masked editing, and one local/external
  reference adapter. Keep provider code feature-gated where dependencies or binary
  size threaten the default artifact. Do not freeze model IDs in the schema.

- [ ] **5. Add generation with candidate review.** Generate one or more bounded
  candidates at an explicit size/aspect ratio, validate and hash all results, show
  cost/usage when reported, and commit only selected candidates. Rejected
  candidates live in bounded temporary storage and never create document history.

- [ ] **6. Add source-aware editing and variations.** Send explicitly selected
  image/reference nodes with roles, preserve their immutable hashes, and place the
  accepted result as a sibling, replacement, or new version. Replacement preserves
  frame geometry, transform, clipping, masks, effects, and stacking unless the
  user deliberately chooses a new placement.

- [ ] **7. Add mask-aware inpaint and object removal.** Reuse v0.7.2 selections
  and raster/vector masks, rasterizing a request mask through the authoritative
  renderer with declared polarity and dimensions. Preview the exact disclosed
  source and mask. Returned pixels never overwrite areas outside the intended
  region without a visual diff and explicit full-frame acceptance.

- [ ] **8. Add outpaint and generative canvas extension.** Require an explicit
  target rectangle, anchor, and resulting pixel budget. Keep original pixels
  attributable, show changed/added regions, and preserve placement in scene
  coordinates. Crop-back remains non-destructive.

- [ ] **9. Add focused cleanup operations.** Normalize remove-background,
  background replacement, restoration, denoise/deblur, and upscale into the same
  recipe/run flow. Prefer a returned alpha mask when available so edges remain
  reusable and editable. Never present a provider's semantic rewrite as lossless
  cleanup.

- [ ] **10. Add reusable recipes and controlled regeneration.** Save portable
  recipes as document styles or data-only `.penpreset` entries. Regeneration shows
  the resolved profile/model, disclosed inputs, parameter compatibility, estimated
  request count, and reproducibility limits. It creates a new immutable run and
  output; it never mutates an earlier result in place.

- [ ] **11. Integrate batch, diff, history, and packages.** Batch can declare a
  recipe and execute it only with an explicit `--allow-model-call` gate. Dry run
  performs local validation and disclosure planning but makes no billable call.
  Diff summarizes prompt/setting changes without leaking private prompts. Packages
  include accepted outputs and sanitized provenance, never credentials or required
  remote calls.

- [ ] **12. Build editor-native controls.** Add model/profile selection,
  capability-aware controls, prompt privacy choice, exact input disclosure,
  candidate grid, before/after and changed-region views, cancel/retry, usage, and
  provenance inspection. Controls appear in the relevant image-editing context,
  not as a general-purpose autonomous chat surface.

- [ ] **13. Add policy, privacy, and cost safeguards.** Support endpoint allowlists,
  per-call pixel/candidate limits, retry ceilings, concurrency limits, optional
  cost confirmation thresholds, log redaction, proxy/TLS policy, and a strict
  offline mode. Clearly distinguish provider safety refusal from transport and
  validation errors.

- [ ] **14. Harden conformance and failure recovery.** Test every adapter against
  recorded local fixtures without network access in normal CI. Cover malformed
  media, decompression bombs, wrong MIME types, huge outputs, partial streams,
  timeouts, cancellation, redirects, retries, duplicate results, model drift,
  unsupported options, secret leakage, rollback, and crash recovery. Live tests
  are opt-in, credential-gated, non-release checks.

## CLI direction

```sh
pentool ai provider add studio --adapter openai-compatible \
  --endpoint https://example.invalid/v1 --credential-env STUDIO_IMAGE_KEY
pentool ai model refresh --provider studio
pentool ai model list --capability inpaint --json
pentool ai generate campaign.pen hero-concept --provider studio \
  --model image-model-id --prompt-file prompt.txt --aspect 16:9 --candidates 4
pentool ai edit campaign.pen product-clean --source product \
  --kind remove-background --prompt-privacy document --dry-run
pentool ai inpaint campaign.pen product-fix --source product \
  --mask selection:cleanup --prompt-file cleanup.txt --allow-model-call
pentool ai outpaint campaign.pen hero-wide --source hero --width 1920 --height 1080 \
  --anchor center --allow-model-call
pentool ai run accept campaign.pen <run-id> --candidate 2 --replace product
pentool ai provenance campaign.pen --node product --json
pentool ai regenerate campaign.pen cleanup-product-photo --allow-model-call
```

Exact command names are provisional. Mutations use shared transactions, history,
revision guards, page selection, structured errors, and one undo entry. `--dry-run`
never contacts a paid model. Non-interactive execution requires the separate
`--allow-model-call` acknowledgement so an existing script cannot begin spending
money merely because a document gained an AI recipe.

## Acceptance targets

- Configure two providers with different capability sets and use the same portable
  generate recipe where both support it, with unsupported fields rejected before
  submission.
- Remove a product background, refine it through a reusable mask, accept one of
  several candidates, and continue editing it with ordinary v0.7/v0.7.2 tools.
- Inpaint a selected region while the review UI shows the precise source, mask,
  prompt privacy, provider, model, settings, and pixels changed.
- Replace an image asset while preserving its scene placement, crop, transform,
  clipping, effects, stable node ID, and undo history.
- Move the completed `.pen` and package to an offline machine and reproduce the
  accepted render exactly without credentials, adapters, or model availability.
- Cancel or fail a request at every stage without changing the document, leaving
  orphan assets, leaking secrets, or recording a misleading successful run.
- Confirm through automated scans that `.pen`, `.penpkg`, history, JSON output,
  logs, fixtures, and crash artifacts contain no configured credential values.

## Explicitly deferred

- Autonomous agents that inspect or redesign an entire document without explicit
  scoped inputs and user-reviewed mutations.
- Silent live inference during render, export, package install, document open, or
  thumbnail generation.
- A Pentool-hosted model gateway, billing account, credential escrow, or mandatory
  cloud service.
- Training, fine-tuning, LoRA management, dataset collection, and model downloads.
- Claims of pixel reproducibility across provider model updates. Materialized
  output hashes, not seeds or prompts, are authoritative.
- Video generation, audio generation, general text/chat models, and arbitrary tool
  execution; v0.7.3 is intentionally limited to image creation and image editing.

## Acceptance demo

Open a product campaign and select one photo. Use a BYOK provider to remove its
background, refine the returned mask with v0.7.2 tools, outpaint a wider hero, and
generate four background candidates from a reusable recipe. Accept one candidate,
apply ordinary curves and effects, then inspect a diff and complete provenance.
Switch to a second provider and regenerate a variation after reviewing capability
differences and exact data disclosure. Finally disconnect the network and render,
package, copy, reopen, undo, and export the accepted campaign identically, with no
provider credentials stored in the project.
