# CLI automation and rendering API

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [Agent discovery and safe editing](#agent-discovery-and-safe-editing)
- [Agent rendering API](#agent-rendering-api)

## Agent discovery and safe editing

Agents no longer need to parse or rewrite a complete `.pen` document to make a
small change. Search returns stable IDs, bounds, stacking indexes, and layer state:

```sh
pentool tree artwork.pen
pentool search artwork.pen composer
pentool search artwork.pen "Open desktop app" --kind text
pentool tree artwork.pen --kind text --layer typography
```

Target an exact layer and object ID. Unspecified properties are preserved, and
each mutation retains the prior document in recoverable history:

```sh
pentool object artwork.pen set composer --layer ui --fill "#1c1c1c"
pentool object artwork.pen rename composer --layer ui --new-id prompt-box
pentool object artwork.pen duplicate prompt-box --layer ui --new-id prompt-copy
pentool object artwork.pen move-to-layer prompt-copy --layer ui --target-layer variants
pentool object artwork.pen reorder prompt-box --layer ui 0
```

For all-or-nothing edits, provide a JSON array of operations. `--dry-run` validates
and summarizes without writing. A committed batch records the prior revision in bounded hidden history. Pass `--revision` from a prior dry
run to reject the batch if the file changed in between.

```sh
pentool batch artwork.pen operations.json --dry-run
pentool batch artwork.pen operations.json --revision 0123456789abcdef
```

On v4/v5 documents `put-shape`, `put-path`, and `put-text` fail on an existing ID by
default. Add `"mode":"replace"` to an operation, or pass `--upsert` to `batch`, to
replace the node in place (stacking position is kept; groups are never replaced).
Re-running the same batch with `--upsert` is therefore idempotent.

### See the result in the same call

Add `--preview` to any command that changes a document, or to its `--dry-run`.
After the commit, Pentool renders the changed page with the normal export
renderer, at 1024 px on the long edge, and adds a top-level `preview` object to
the command's JSON result:

```sh
pentool shape artwork.pen rect badge --layer ui --x 10 --y 10 --w 50 --h 50 --fill "#ff0000" --preview
```

```json
"preview": {"path": ".pentool/preview/artwork-cae6baff68dd.png", "page": "page-1",
            "width": 1024, "height": 768, "scale": 3.2,
            "revision": "cae6baff…", "dry_run": false}
```

`revision` is the revision the same call wrote, so the image never shows a
stale state. `--preview-size PX` picks the long edge (1 to 8192),
`--preview-out PATH` writes the image to a chosen path, and `--page` picks the
page (otherwise the first page that changed). `PENTOOL_PREVIEW=1` turns previews
on for every mutating command. Images in `.pentool/preview/` are derived data:
the 32 newest are kept and older ones are removed. A preview that cannot be
rendered or written is reported as `"preview": {"error": ...}`; the edit itself
still succeeds. Read-only commands print exactly what they print without it.

A preview zooms in on what changed. When the command added or edited objects
that cover less than half of the page, the image shows their bounds plus
padding at up to 8x detail, and the result gains `focus` (the object IDs and
the area in document units) and `thumbnail` (a 256 px image of the whole page,
written beside the preview with a `-page` suffix). Every pixel is the export
render at the reported `scale`, cropped. `--focus ID` (repeatable) zooms in on
chosen objects instead; `--no-focus` always shows the whole page. More than 64
changed objects, or none that can be identified (a page rename, say), show the
whole page.

`pentool preview DOC` takes the same look without changing anything: it
prints `{"preview": {...}}` with the same fields for the document as it is on
disk. It shows the whole page unless `--focus ID` is given, and takes
`--page`, `--max-size PX` (default 1024) and `--out PATH`:

```sh
pentool preview artwork.pen --focus badge --max-size 512
```

`--annotate` (with `pentool preview` or any mutating command; it implies
`--preview`) draws each object's bounds and ID on the preview image, so an
agent can name what it sees ("move `cloud-12` left") without a `tree` call.
The result gains `annotations`: `labels` lists `id`, `color` and the box
`x`, `y`, `width`, `height` in preview pixels, in document order (later labels
are drawn on top), and `truncated` says whether objects were left out. A
whole-page preview labels every object on the page, a focused one labels the
focused objects; at most 50 labels are drawn. The overlay exists only in the
preview image: the thumbnail, exports and the document never carry it.

`--compare REV|FILE|before` (it also implies `--preview`) checks a change by
numbers instead of by eye. It renders the same view of another state at the
same scale and adds `compare` to the preview: `against`, its `revision`,
`changed_pixels`, `changed_pct` and `changed_bbox` (preview pixels, or `null`
when nothing changed). `before` is the state the command started from (for
`pentool preview`, the start of the last recorded change); a 64-digit hex
value is a revision from the document's history; anything else is a `.pen`
file. When pixels changed, `diff.path` names an image beside the preview with
the changed pixels in red over a faded copy:

```sh
pentool shape artwork.pen rect badge --layer ui --x 10 --y 10 --w 50 --h 50 --fill "#ff0000" --compare before
```

Previews and exports of composite (v6) documents keep each finished layer
surface in `.pentool/cache/` beside the document, keyed by a hash of the layer
and everything that affects how it renders. The next CLI call renders only
the layers that changed, so edit-then-preview loops get faster after the first
call; the output bytes are the same with or without the cache. The cache is
bounded to 256 MB (least recently used entries go first), is dropped when the
`pentool` binary changes, and treats a damaged entry as a miss. `--no-cache`
or `PENTOOL_CACHE=0` turns it off; deleting the directory is always safe.

`pentool session --jsonl` runs many commands in one process, so an agent
pays process start once and keeps fonts and rendered layers warm. Each stdin
line is a JSON array of the arguments that would follow `pentool`, or an
object with `args` and an optional `id`; each reply is one line:
`{"ok":true,"result":...}` (or `"text"` for plain output) or
`{"ok":false,"error":{...}}` with the same error object as `--json`. A bad
line gets an error reply and the session continues. Documents are read and
written exactly as single commands do, with the same locks, history and
`--if-revision` guards; `serve` and nested sessions are refused.

Rendering splits gradient fills, blurs and layer blending across the CPU's
cores (at most 16). Each row is computed exactly as on one thread, so the
bytes never depend on the thread count; `PENTOOL_THREADS=N` sets it, and
`PENTOOL_THREADS=1` renders on one thread.

On Windows the list of installed fonts is also kept between calls
(`%LOCALAPPDATA%\pentool\fonts.bin`), so text renders do not open every
system font file. It is used only while a
listing of the font files in those folders (names, sizes and times) is
unchanged, and it is rebuilt for each new `pentool` binary; `--no-cache` and
`PENTOOL_CACHE=0` skip it. Pages without text never look at installed fonts.

```sh
printf '%s\n' '["tree","art.pen","--limit","5"]' \
  '{"id":2,"args":["shape","art.pen","rect","badge","--layer","ui","--x","10","--y","10","--w","50","--h","50","--preview"]}' \
  | pentool session --jsonl
```

`export --max-size PX` does the same sizing for exports: it picks the scale so
the longest page edge is at most `PX` pixels (1 to 8192; with `--all-pages`
the largest page decides) and reports it as `"scale"` beside `"max_size"`. It
replaces `--scale`, so the two cannot be combined:

```sh
pentool export artwork.pen artwork.png --max-size 800
```

Unknown JSON extension fields follow objects through rename, duplicate, move,
and reorder. Locked source/target layers reject mutations. Object matching never
guesses: mutation commands require exact layer and object IDs. Inside a layer,
paths render first and text renders above them; `tree` exposes explicit
`draw_order` values. In the browser, the searchable Layers panel uses the same
Rust editing operations for object rename, duplicate, reorder, and delete.

## Agent rendering API

The server accepts a complete `.pen` document and returns a rendered file:

```sh
curl -X POST http://127.0.0.1:4711/api/render/png \
  -H "content-type: application/json" \
  --data-binary @logo.pen \
  --output logo.png
```

Endpoints:

- `GET /api/health`
- `POST /api/render/png`
- `POST /api/render/svg`

The API is intentionally simple: an AI agent can create or edit the human-readable path data, preserve layers, and ask the same binary to render it. Canvas dimensions are capped at 16,384 px and request bodies at 16 MiB.
