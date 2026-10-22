# Pentool v0.25.0

Optional image generation and editing in Studio, plus the accumulated internal
0.24.2 bug fixes. No new document fields or dependencies.

## Studio AI

- Connect and validate providers using credential references; browser forms never
  receive an API key. Provider settings require the collaboration owner role.
- Review requests offline, then explicitly confirm provider, model, host, call
  count, prompt hash and source hashes before contacting a paid model.
- Generate images, preview candidates, accept into the selected layer, replace a
  selected image, or discard a run. Runs survive reloads and remain bounded to 16.
- Accept with revision guards and source/lock checks using the CLI transaction
  engine. Each acceptance is one undoable document-history step. Selected accepted
  images show their provider/model provenance in the AI panel.
- Gemini image variations and background removal; local magenta key-out is offline.
- Cancel between provider calls and before storing candidates. A request already
  sent may still be billed and finishes or times out before the worker exits.
- `PENTOOL_AI=off` disables operations and hides the AI entry points.

## Bug fixes

Includes all fixes recorded in [the internal 0.24.2 notes](release-v0.24.2.md):
parent-page rendering, spread facing values, batch paint validation, hidden-node
export, safer provider credential input, negative geometry arguments, serve/import/
extension diagnostics, and CLI help corrections.

Spot-only `color separate --composite` now uses the configured or explicit CMYK
output destination to render spots over process paper (#397).

## Limitations

Masked inpaint/outpaint and spending budgets remain deferred. Model availability,
pricing and support depend on the configured provider. CPU rendering remains
normative. Placed native table rendering remains a known gap.
