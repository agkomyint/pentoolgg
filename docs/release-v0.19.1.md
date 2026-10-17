# Pentool v0.19.1

This user-directed Studio redesign is included in the patch release after
v0.19.0 had shipped. The release keeps the `.pen` document format compatible
and preserves the existing CLI while making Studio workflows more coherent.

## Highlights

- Redesign Studio navigation around Design, Photo, Layout, Packaging, Print,
  and Extensions, with contextual tool controls and a combined vector and paint
  toolbar.
- Add a project welcome screen with resume/open/create flows and common canvas
  sizes.
- Support JPEG and PNG photo import into the served project, non-destructive
  image adjustments, and placing the developed image onto the active page.
- Expose document export actions and keep layers visible while editing.
- Fix 12 user-reported issues merged into the release, including section-list
  pagination, layout handling, raster inspection, export error details,
  insertion validation, and consistent mutation summaries.

The complete scope and acceptance criteria are in
[`roadmap/v0.19.1/TASKS.md`](roadmap/v0.19.1/TASKS.md).

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
[`roadmap/v0.19.1/TASKS.md`](roadmap/v0.19.1/TASKS.md). Specialized photo,
paint, and 3D stages remain in the same project shell.
