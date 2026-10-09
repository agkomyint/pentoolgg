# Pentool v0.11.1

Bug fixes for v0.11.0, found by developing a real camera DNG (`colorchart.dng`
from syoyo/tinydng: Canon EOS 550D, Magic Lantern). No format changes; documents
written by v0.11.0 open unchanged.

## Fixes

- `raw add` no longer refuses DNGs whose BaselineExposure (tag 50730) is the
  rational 0/0. Magic Lantern writes the tag this way when it is unset. The tag is
  now read as absent, and the result carries a `warnings` list naming it. A zero
  denominator in any tag the develop depends on, such as ColorMatrix, is still
  refused.
- Page export no longer develops each `photo` node from scratch on every render.
  Page renditions now go through the photo cache (`.pentool/cache/photo/`), keyed
  by the develop settings and size, so nodes that show the same variant share one
  develop and repeat exports reuse it. The cached output is byte-identical to a
  fresh render. With three nodes of the 1734×1156 test DNG, export took 2.8 s
  before this change, 0.9 s on the first run and 0.3 s on repeat runs.
- `photo place DOC PHOTO --layer LAYER [--variant ID] [--id ID] [--x --y] [--width
  --height] [--fit] [--position X,Y] [--dry-run] [--if-revision]` adds a `photo`
  node through a transaction. Version 0.11.0 documented photo nodes but gave no
  CLI to create one. Missing size comes from the developed size, keeping the
  aspect ratio. Missing photos or variants, missing layers, locked layers and IDs
  already in use are refused, and the document is left unchanged.
- Clearer errors:
  - a grain effect without a seed suggests `{"amount": 25, "seed": 1}`;
  - a boolean `monochrome` points to `monochrome.enabled`;
  - `crop` given inside geometry points to the top-level `crop` group.

## Documentation

- [`docs/photography-v1.md`](photography-v1.md) no longer lists `photo import` or
  `photo local add`, which were never implemented. Imports go through `raw add`,
  and local adjustments are made with `raw develop --set 'local=[...]'`, for which
  there is a worked example.
- The CLI section now says which commands take `PHOTO --variant`, which take
  positional selections, and which take `PHOTO/VARIANT`.
- The `monochrome`, `crop` and grain `seed` rows now match validation.
- The page placement section covers `photo place` and the rendition cache.

## Known limitation

Pentool trusts the black level a DNG declares. In the reported file, the declared
level is 2056, but the masked optical-black pixels measure about 2047–2050. This
crushes red in deep shadows, so the darkest patches show a green/cyan cast.
Estimating the black level from masked areas would be a new feature, so it is not
part of a patch release.
