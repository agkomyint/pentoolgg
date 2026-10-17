# Pentool v0.18.1

This user-directed Studio UI redesign is an exception to the patch-only bug-fix
policy after v0.18.0 had shipped. Future releases should use the documented
semantic versioning policy.

## Studio workflow redesign

- Welcome screen to continue a project, open `.pen`, or create a common or
  custom-sized canvas.
- Grouped Design, Photo, Layout, Packaging, Print, and Extensions navigation;
  Canvas, Images, Composite, and Paint are contextual Design modes.
- One application bar with contextual tool settings, a combined vector/paint
  tool rail, and a resizable dock with layers visible alongside the inspector.
- Persistent visited canvases, shared Design/Paint zoom and framing, and bounded
  preview reuse for unchanged documents. Hidden canvases pause editing shortcuts.
- DNG/JPEG/PNG import into a shared served project, rendered-image tone/color/
  detail/effects/crop edits, and placement of a developed photo on the active
  page. Camera-specific RAW controls remain DNG-only.
- Main export menu for current-page PNG/SVG and document PDF/EPUB.

The project roadmap lists scope, acceptance criteria, and limits in
[`roadmap/v0.18.1/TASKS.md`](roadmap/v0.18.1/TASKS.md). Specialized photo,
paint, and 3D stages remain in the same project shell.
