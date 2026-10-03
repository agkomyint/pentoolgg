# Pentool v0.6.2

This release moves everyday authoring above raw SVG paths while retaining the
deterministic offline renderer and legacy v1–v3 compatibility.

Highlights:

- ordered v4 scenes, explicit migration, semantic primitives, nested groups, and
  local components with fallback snapshots;
- one validated transaction boundary with revision guards, dry runs, atomic
  writes, project-local content-addressed history, undo, redo, restore, and prune;
- complete-scene batch construction, named design tokens, scoped replacement,
  measured layout/grid operations, and deterministic bounded text;
- structured JSON errors, natural negative numeric arguments, structural and
  visual diff artifacts, compact/pretty formatting, multi-page PNG/SVG export,
  and vector/searchable-text PDF output;
- v4 browser preview and shared scene/history APIs.

See [authoring-v0.6.2.md](authoring-v0.6.2.md) for task-oriented examples and
[pen-format-v4.md](pen-format-v4.md) for the format contract.
