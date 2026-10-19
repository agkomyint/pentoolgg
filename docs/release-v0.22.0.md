# Pentool v0.22.0 (unreleased)

Minor release. No `.pen` format or CLI changes.

## Studio

- Live document: an open Studio tab now updates by itself when the served file
  changes, whether the edit came from the CLI, an agent, another tab or a
  collaboration peer. Unsaved edits in the tab are merged with the incoming
  change instead of being overwritten, and there is no "Changed on disk" banner.
- Photo: an "Add to canvas" button in the Photo toolbar and panel places the
  selected photos, with their edits, on the active layer and opens Design.
  Several selected photos are placed with a small offset. When it is disabled,
  its tooltip says why. Later Photo edits update the placed photo.
- Photo: a project with no photos yet opens with an empty grid instead of
  `[missing-resource] the document has no photography catalog`, so the first
  JPEG, PNG or DNG can be imported from Studio.

- Photo: the Layers dock has a Photos tab with a thumbnail strip of the
  catalog. Drag thumbnails onto the canvas to place them where they are
  dropped, or onto a page thumbnail to place them in the middle of that page.
  Double-click a thumbnail to add it to the current page.
- Photo: placing a photo on a page that had not been saved yet failed with
  `[missing-resource] page page-N not found`. Studio now saves the canvas first.
- Pages: the page strip at the bottom has a compact mode (the chevron button on
  the right) that shows page name chips instead of thumbnails. Studio remembers
  the choice.

## Server

- New `GET /api/document/events` server-sent event stream that reports the shared
  document's revision on connect and on every change. Works with or without
  `--collab` (view access required when collaboration is on). See
  `docs/PROTOCOL.md`.
