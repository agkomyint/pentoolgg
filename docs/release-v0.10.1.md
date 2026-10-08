# Pentool v0.10.1

Bug-fix release for the v0.9.0 audit (PEN-090-001..018). No new features and no `.pen`
format change. Behaviour that was previously unsafe now fails with a structured error and
leaves the document byte-for-byte unchanged.

## Document safety
- `pentool new` refuses to replace an existing file; `--overwrite` records the previous
  document in history so `pentool undo` restores it (PEN-090-014).
- Canvas width/height must be integers in 1..=16384 for `new`, `canvas` and `page add`
  (PEN-090-015).
- Invalid canvas backgrounds, fill/stroke fallback colors (grammar: `none`, `#RGB`,
  `#RGBA`, `#RRGGBB`, `#RRGGBBAA`) and blank layer/object IDs are rejected when a v4+
  command would introduce them; already-invalid documents stay editable (PEN-090-016,
  PEN-090-017).
- `export` help and errors name PDF (PEN-090-018).

## AI tooling
- Endpoints reject credential-like query parameters and fragments and never echo them
  (001). `PENTOOL_AI=off` hides `ai` from `--help` (002). Capability errors no longer
  recommend a provider that lacks the capability (003). `--dry-run` enforces
  `PENTOOL_AI_ALLOW_HOSTS` (004). Opaque background-removal results fail with
  `keyout-failed` and coverage is recorded (005). Duplicate candidates are reported
  (`requested`/`unique`/`warnings`) and all usage is kept (006). Windows credential files
  must be under the user profile and outside the project (007). Oversized credential
  files keep code `limit-exceeded` with a fix (008). Malformed limit variables fail
  closed (009). `--replace` rejects stale sources (`--allow-stale-source` overrides)
  (010), runs are scoped per document (011), `--replace` honors layer/node locks (012),
  and `--default-model provider/model` is accepted (013).

Runs created by v0.10.0 and earlier in the shared `.pentool/ai-runs/` folder are not
listed by v0.10.1; regenerate if you still need them.
