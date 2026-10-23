# Selecting and cutting out

Pentool v0.27.0 can select a subject in a photo, cut it out of its background and
place it on a page, from the command line and from Studio. Everything is
deterministic (same input, same bytes), needs no model or network, and every
command supports `--dry-run`.

## The agent recipe

1. **Look.** Render a preview of the page or image (`--preview`) and read pixel
   coordinates off it. All cutout coordinates are page units, the same ones a
   rendered preview shows.
2. **Outline.** Give a rough shape around the subject: a box, a polygon, or a
   wand seed point.
3. **Refine and cut.** `cutout` snaps the outline to image edges and lifts the
   subject into a new transparent raster layer. The source is never changed.
4. **Place.** The new layer sits where the subject was; move or scale it with the
   normal object commands.

```sh
pentool raster board.pen cutout photo --id car --box 120 140 400 190 --refine 25 --decontaminate
pentool raster board.pen cutout sky-photo --id subject --wand 10 10 --gradient 12 --sample 5 --invert --refine 6
```

`--refine BAND` snaps the shape to edges: pixels more than BAND inside are subject,
more than BAND outside are background, and the ring between is decided by edges
and color. `--fg '[[x,y],...]'` and `--bg '[[x,y],...]'` pin pixels you know are
subject or background (a single point is a 3x3 dab); the same pixel in both is an
error. `--shrink N` pulls the edge in, `--feather N` softens it and
`--decontaminate` recolors the soft edge from the opaque interior so no background
fringe remains. The result lists the new layer's `bounds`, selected pixel counts,
hashes and `warnings` (for example when the selection reaches the image edge).
The new layer is trimmed to the subject's bounds.

`--dry-run` plans the cutout and reports the same numbers without writing.

## The magic wand

`raster DOC select-wand ID --x X --y Y` selects a region of similar color. Two
options make it work on photographs:

- `--sample 3|5|11` averages a square around the seed so one noisy pixel does not
  decide the region.
- `--gradient N` compares each pixel with the neighbor it grew from instead of the
  seed, so a sky, studio sweep or shaded surface selects as one region.
  `--leash N` still caps how far any pixel may drift from the seed.

## Edge refine and modifiers

`select-refine ID [--band N] [--core N] [--fg P] [--bg P]` snaps the active
selection to image edges. `select-modify ID --op feather|expand|contract|smooth|border|grow|similar|invert`
changes it. Selections can be saved (`select-save`), reloaded (`select-load`) and
deleted (`select-delete`). `lift`, `move-pixels`, `transform-pixels` and `paste`
work on the selected pixels. All of these are available through
`POST /api/raster` and `raster batch` as well.

## Document selections and cropping one image

`pentool selection` queries run on the rendered page. They accept the raster
selections above (`{"kind":"raster-selection","layer":"p"}`, optionally with
`"name"` for a saved one) plus `invert`, `grow`, `shrink` and `feather`, so a
raster selection can be saved as a mask or used to measure and analyze.

`selection crop DOC --query Q --node IMAGE_ID` crops one image to the selection
bounds without resizing the page (the image needs `fit: fill` and no transform;
`image set DOC ID --fit fill` first). Without `--node`, `selection crop` crops the
page as before.

## In Studio

Select a raster layer and pick the Marquee, Lasso or Wand tool in the Raster panel.
The Wand has sample size and gradient controls. The panel's Invert, Feather,
Expand, Contract and Smooth buttons change the selection, **Refine edge** snaps it
to the subject, and **Copy to layer** / **Cut to layer** lift the selected pixels
into a new layer. Each is one undoable step.

## Limits

Selection is classical image processing: it needs a rough outline from you or an
agent and does not recognize objects by itself. Hair and fur edges are
approximations, not matted alpha.
