# FOX ONE poster

A 1200x1600 poster whose art was generated with Gemini image models and
composed, cut out, glowed and typeset entirely with Pentool (v6 document).

Generation (outside Pentool; Gemini returns JPEG without alpha):

1. Background: `gemini-2.5-flash-image`, "deep-space nebula, no text", 3:4.
2. Subject: same model, "astronaut fox, flat solid magenta #FF00FF background".

Pentool steps: add both images, `selection color-range` on the magenta,
`mask create/attach/apply --invert` for the cutout, `outer-glow` and
`drop-shadow` effects, a `vibrance` adjustment scoped to the fox, and
left-aligned `text-box` titles with an `outer-glow`.

Export: `pentool export examples/fox-one-poster.pen examples/fox-one-poster.png`.
Images are embedded; no API key or provider data is stored in the document.
