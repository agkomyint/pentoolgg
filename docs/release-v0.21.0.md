# Pentool v0.21.0 — Studio editing

A Studio usability release: selection, direct manipulation, and navigation now
work the way designers expect from desktop editors. It also carries every fix
from the unreleased 0.20.1 patch (`docs/release-v0.20.1.md`). No `.pen` format,
CLI, or dependency changes; Studio edits go through the existing server routes.

## Studio

- **Multi-selection.** Shift-click adds or removes objects, Ctrl/Cmd+A selects
  everything on the page, and a selection of several objects moves, resizes,
  aligns, and groups together.
- **Accurate picking.** Clicks hit the drawn shape instead of its bounding box.
  Clicking selects the outermost group first; double-click steps into a group.
  Hovering outlines the object under the pointer.
- **Smart guides.** Dragging snaps to the edges and centers of nearby objects and
  the page, with a badge showing position and size.
- **Context menu and clipboard.** Right-click the canvas for common commands;
  copy, cut, paste, and duplicate objects with the usual shortcuts.
- **Command palette.** Ctrl/Cmd+K searches every command; `?` opens the shortcut
  sheet.
- **Layers.** Drag to restack, nest into a group, or move to another layer;
  double-click to rename; hovering a row highlights the object on the canvas and
  the reverse. Locked layers and legacy documents keep their existing rules.
- **Text in place.** Double-click a text object to edit it directly on the
  canvas.
- **Start screen.** Canvas presets grouped as Social, Screen, and Print, a
  custom size with an orientation swap, a thumbnail card to continue the open
  project, and drag-and-drop to open a `.pen` file.

## Fixes

- Bursts of previews (page strip, thumbnails, zoom) no longer fail with
  `[limit-exceeded] preview workers busy`: Studio keeps its requests under the
  server's worker limit and retries briefly when the workers are busy.

## Notes

- Server previews of pages with many objects are slower than single-object
  pages because each object is composited at page size; a bounded-region
  renderer is planned. Run the release binary, not a debug build, for
  interactive use.
- In-browser ("Local render") previews still cover format v6-v9 documents only;
  newer documents render on the server.
