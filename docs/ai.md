# Optional image models (`pentool ai`, v0.9.0)

Everything here is optional. Without a configured provider Pentool never touches the
network, and `PENTOOL_AI=off` disables the `ai` commands entirely. Opening,
validating, rendering, and exporting a document never call a model.

## Zero-touch setup (agents)

```sh
export GEMINI_API_KEY=...            # or GOOGLE_API_KEY / OPENAI_API_KEY
pentool ai setup --from-env          # idempotent; stores the variable *name*, never the key
pentool ai doctor --check            # one non-billable model-list call
pentool ai resolve --capability generate
```

Or with no prior state: `pentool ai connect --name studio --adapter openai-compatible
--endpoint https://host/v1 --credential-env STUDIO_KEY`. Keys are references
(`--credential-env NAME`, `--credential-file PATH`); `--api-key` is rejected.
`--ephemeral` / `PENTOOL_AI_EPHEMERAL=1` resolves from the environment and writes
nothing. Generic variables: `PENTOOL_AI_PROVIDER`, `PENTOOL_AI_ENDPOINT`,
`PENTOOL_AI_KEY`, `PENTOOL_AI_KEY_FILE`, `PENTOOL_AI_MODEL` (`provider/model`),
`PENTOOL_AI_ADAPTER`. Config lives in the platform config dir (`ai.json`, override with
`PENTOOL_AI_CONFIG`).

Errors under `--json` carry a stable `code` and a `fix` naming the corrective step.

## Adapters

| adapter | capabilities | notes |
| --- | --- | --- |
| `gemini` | generate, edit, remove-background | `generateContent`; key sent as `?key=`; outputs are opaque (no alpha) |
| `openai-compatible` | generate | `/images/generations`, `b64_json` only; URL results are never fetched |

Endpoints must be `https` (plain `http` only for loopback), cannot embed credentials,
never follow redirects, and can be restricted with `PENTOOL_AI_ALLOW_HOSTS`.

## Generate, review, accept

Every model call requires `--allow-model-call`; `--dry-run` prints exactly what would
be sent (provider, host, prompt hash, source hashes) and never connects.

```sh
pentool ai generate poster.pen hero --prompt "a fox in a neon city" --aspect 3:4 \
  --candidates 2 --allow-model-call
pentool ai run list poster.pen
pentool ai run accept poster.pen RUN --candidate 1 --id hero      # new image node
pentool ai run accept poster.pen RUN --candidate 2 --replace hero # swap pixels, keep placement/effects
pentool ai run discard poster.pen RUN
```

Candidates are stored under `.pentool/ai-runs/` (newest 16 kept) and the document does
not change until `accept`, which is one transaction with `--dry-run` and `--if-revision`.
Accepted nodes carry an `ai` provenance object (provider, model, prompt hash, run); the
prompt text and credentials are never written to the document.

Limits: `PENTOOL_AI_MAX_CALLS` (default and cap 8), `PENTOOL_AI_MAX_PIXELS`,
`PENTOOL_AI_TIMEOUT_SECS`.

## Background removal

Models do not return alpha. Two supported routes:

- `pentool ai edit doc.pen cut --source IMG --kind remove-background --allow-model-call`
  asks the model for a flat `#FF00FF` background and keys it out locally.
- `pentool ai keyout doc.pen cut --source IMG --key '#FF00FF'` is the offline route for
  any image already on a flat colour: no model, no network.

## Not in 0.9.0

Inpaint/outpaint with masks, recipe/regenerate, the editor "Connect a provider" UI, the
AI SDK bridge and external-adapter protocol, OS keyring storage, and a spend budget
(`PENTOOL_AI_MAX_COST`) are planned and not implemented.
