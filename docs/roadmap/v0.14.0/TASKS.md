# v0.14.0 — Keyframe animation and lean video export

Make `.pen` documents move. A page gains an optional timeline, and any node can
animate its properties through keyframes. Pentool renders those frames
deterministically and exports them as a PNG sequence, an animated GIF, or an MP4.
It writes all three with its own Rust encoders, so it stays one offline binary
with no ffmpeg, codec service, GPU or Node.js runtime.

Audio ships in v0.15.0, but this release must not block it. The time base, the
track model, the MP4 writer and the editor timeline are all designed here for
audio tracks, even though v0.14.0 writes video only.

This is a minor release because it adds a format version, commands and editor
features. Patch releases in the v0.13 line remain limited to bug fixes and
documentation.

## Product contract

- The Rust renderer remains authoritative. A frame is an ordinary page render
  with animated values applied, so every node kind, effect, mask, photo node and
  raster layer animates through the same path as a still export.
- Time is exact. Times are stored as rational numbers of a declared time base,
  never floating-point seconds. Frame *n* at a rational frame rate *r* is sampled
  at exactly *n*/*r*, so the same document gives the same frames on every
  platform.
- A document with no timeline renders, exports and round-trips exactly as before.
  An animated document's still export is the frame at its declared poster time,
  which is 0 by default.
- Encoded output is deterministic. The same document, settings and engine
  version produce a byte-identical GIF and MP4 on Linux, Windows, Intel macOS and
  Apple Silicon. Encoders use integer arithmetic wherever the format allows.
- Keyframe edits are ordinary mutations. Dry run and commit share one planner,
  and failed, cancelled, stale or locked edits leave the document byte-for-byte
  unchanged.
- Export work is bounded. Limits on frame count, duration, pixel size, total
  pixels, output bytes and memory are checked before rendering starts.
  Cancellation leaves no partial output file.
- Lean is a requirement. The GIF, MP4 and H.264 writers must add less than 1 MiB
  to the release binary, with no new C toolchain dependency. Each new dependency
  must meet the AGENTS.md bar.
- Audio-ready: the timeline, track model and MP4 container accept audio tracks in
  their schemas and data structures now. v0.14.0 refuses audio content clearly as
  `[unsupported-capability]` instead of dropping it.

## Animation model

```json
{"timeline": {
  "time_base": [1, 600],
  "duration": 3600,
  "frame_rate": [30, 1],
  "poster": 0,
  "loop": true,
  "tracks": [
    {"id": "logo-slide", "kind": "property", "target": "logo", "property": "x",
     "keys": [
       {"t": 0,    "value": -200, "easing": "ease-out"},
       {"t": 600,  "value": 120,  "easing": "linear"},
       {"t": 3600, "value": 120}
     ]},
    {"id": "logo-fade", "kind": "property", "target": "logo", "property": "opacity",
     "keys": [{"t": 0, "value": 0}, {"t": 300, "value": 1}]}
  ]
}}
```

- `time_base` is a rational number of seconds per tick. The default of 1/600 is
  divisible by 24, 25, 30, 50 and 60 frames per second. v0.15.0 converts audio
  sample positions into this time base exactly; it does not round them to frames.
- A track has a `kind`. v0.14.0 defines `property`, reserves `audio` and
  `marker`, and refuses unknown kinds.
- Animatable properties are listed in a table in the specification. Each entry
  gives the value type and the interpolation. Numbers interpolate linearly,
  colors in linear-light RGB, transforms by decomposition, and paths only when
  their command structures match. Anything unlisted is refused, never silently
  ignored.
- Easing: `linear`, `step-start`, `step-end`, `ease`, `ease-in`, `ease-out`,
  `ease-in-out` and `cubic-bezier(x1,y1,x2,y2)`. Each is evaluated by a
  documented fixed-iteration solver, so results are bit-identical everywhere.
- Before the first key, a track holds its first value; after the last key, it
  holds its last value. A static property and an animated property never fight:
  while a track exists, it owns that property.

## Must ship, in order

- [ ] **1. Freeze the animation and video specification and threat model.**
  Specify:
  - the time base, rational arithmetic, the timeline, tracks and keys, easing
    math, and the table of animatable properties and how each interpolates;
  - poster frames and how static export behaves;
  - the next `.pen` format version and its migration in both directions;
  - the GIF and MP4 profiles, the H.264 subset, rate control, and the limits;
  - the reserved `audio` and `marker` tracks, and the MP4 multi-track layout
    v0.15.0 will fill.

  Record the codec licensing position for H.264 as an explicit release gate, as
  a decision by the project owner, before the encoder ships. Publish schemas
  plus valid, invalid, adversarial and cross-platform fixtures before adding
  mutation commands.

- [ ] **2. Add the timeline to the format, with migration.** Add the new format
  version with an optional page `timeline`. Detect the version before
  deserializing. Migrating up is lossless. Migrating down is refused while
  animation exists; without animation, it is exact. Validate that track targets
  exist and are unique, that properties are allowed, that keys are sorted and
  unique by time, and that value types and limits hold. Deleting or renaming an
  animated node must update or refuse its tracks, never leave them dangling.

- [ ] **3. Add deterministic frame sampling.** Implement `sample(page, t)`,
  which returns the page with animated values applied. Rendering goes through
  the shared scene path. Cover:
  - nested groups, transforms, masks and clipping;
  - component instances and their overrides;
  - photo nodes, raster layers and effects.

  Add `pentool frame DOC --time T --out frame.png` and `tree --time T`.
  Golden tests pin sampled values and rendered frames for every easing and
  property type.

- [ ] **4. Add keyframe editing to the CLI and the agent API.** Add
  `anim key add|set|remove|list`, `anim track add|remove|list`, and `anim
  timeline set` (duration, frame rate, poster, loop). Each runs through the
  shared transaction and history path with dry run and `--if-revision`. Also:
  - tree and search report `animated: true` and track IDs for nodes, compactly;
  - `anim key list` paginates and filters by node, property and time range,
    with exact `matches`, `returned`, `offset`, `limit` and `has_more`.

- [ ] **5. Export PNG frame sequences.** `pentool export DOC frames/ --format
  png --fps N [--from T --to T] [--scale S]` writes zero-padded, deterministic
  file names and a manifest giving the frame count, time base and frame times.
  Frames render in parallel within a memory budget. Unchanged frames are detected
  from the sampled scene, not from pixels, and are reused. The output directory
  is staged and renamed into place, so cancellation leaves nothing behind.

- [ ] **6. Write Pentool's own animated GIF encoder.** Implement it in Rust:
  - GIF89a with LZW compression;
  - a deterministic palette per frame or across all frames (median cut or
    octree, documented);
  - optional ordered or error-diffusion dithering with a fixed scan order;
  - transparency, a loop count, and frame delays rounded with a running error
    so total duration never drifts;
  - frame differencing: write only the changed rectangle, with the right
    disposal method.

  Tests decode every output with an independent decoder and compare it with
  the reference frames.

- [ ] **7. Write Pentool's own MP4 (ISO BMFF) container writer.** Support
  `ftyp`, `moov`/`mvhd`/`trak`/`mdia`/`stbl` sample tables, `mdat`, and
  `faststart` (the `moov` box before `mdat`). Write exact durations from the
  time base, and use 64-bit offsets for large files. The track list, the
  interleaving of samples in `mdat`, and edit lists are generic over track kind,
  so v0.15.0 adds an audio track without restructuring the writer. Writing
  streams through a temporary file, and memory is bounded regardless of
  duration.

- [ ] **8. Write the H.264 intra encoder.** Implement Constrained Baseline
  H.264 in Rust:
  - SPS and PPS, IDR slices, 16×16 and 4×4 intra prediction;
  - the integer transform and quantization;
  - CAVLC entropy coding and the in-loop deblocking filter;
  - an exact RGB → BT.709 4:2:0 YCbCr conversion with documented rounding, and
    color signalling (VUI) so players show the intended colors.

  This milestone produces a valid all-intra MP4 that plays everywhere.

- [ ] **9. Add timeline-aware P-frames: the lean, fast path.** Add
  P-macroblocks predicted from the previous frame, with motion compensation and
  skip blocks:
  - The scene sampler reports which page regions changed between frames.
    Unchanged macroblocks are coded as `P_Skip` with no search.
  - Translation tracks give exact motion vectors for moving nodes. A small
    bounded refinement search covers everything else.
  - Slices encode in parallel within a frame.

  Acceptance: a typical motion-graphics clip, a mostly static page with moving
  elements, encodes faster than it renders. Its file size is documented against
  the all-intra output.

- [ ] **10. Add rate control and quality settings.** Offer `--quality`
  (constant QP) and `--bitrate` (a deterministic per-frame budget with no
  wall-clock dependence), a keyframe interval, and a maximum output size. Report
  the encoded bytes, average QP, IDR count, skipped-block share and encode time.
  Validate everything before any work starts.

- [ ] **11. Build the editor timeline.** In the browser editor, add:
  - a timeline panel with a time ruler, a playhead, scrubbing and play/pause;
  - one row per track, with keyframe diamonds you can add, move, delete and
    multi-select;
  - easing editing with a curve preview, and a record mode where editing a node
    at the playhead creates or updates a key;
  - in/out range selection, plus duration, frame rate and poster controls.

  Playback uses proxy-resolution frames from the Rust renderer, cached by time
  and settings. Preview may drop frames, but the frame shown at a given time is
  always the authoritative one. Every edit is one undoable change. The panel
  reserves room for the v0.15.0 audio and marker rows.

- [ ] **12. Add an export dialog and progress.** The editor offers PNG sequence,
  GIF and MP4 export with presets: social square, 1080p30, 4K30, and a looping
  GIF. Each shows a size and time estimate from the planner, live progress, and
  cancellation. The server streams progress through the existing protocol. The
  CLI prints the same planner estimate with `--dry-run`.

- [ ] **13. Integrate with history, diff, packages and components.**
  - Structural diff summarizes track and key changes.
  - Packages carry animated documents, and verify reproduces their frames
    offline.
  - Component definitions may contain animation, and instances may override
    track values but not track structure.
  - Batch, locks, page selection and unknown fields behave like other mutations.
  - The photo rendition cache serves animated photo exposure without redeveloping
    every frame.

- [ ] **14. Harden conformance, performance and failure recovery.**
  - Golden frames and byte-identical GIF and MP4 output on all four release
    targets.
  - Decoder conformance: CI decodes every test MP4 with an independent decoder
    (ffmpeg or the JM reference decoder, installed for tests only and never
    shipped) and checks PSNR against Pentool's own renders.
  - Fuzzing of timelines, keys, easing parameters and hostile dimensions.
  - Cancellation at every stage, disk-full handling, and transaction rollback.
  - Benchmarks for 1080p30 and 4K30 export, recorded in `docs/performance.md`.
  - Release smoke tests that export a GIF and an MP4.

## CLI direction

Exact command names are provisional. They must reuse the shared transaction,
planner and structured-error conventions.

```sh
# Define a timeline and animate a node.
pentool anim timeline set promo.pen --duration 6s --fps 30 --loop
pentool anim key add promo.pen logo x --at 0 --value -200 --easing ease-out
pentool anim key add promo.pen logo x --at 1s --value 120
pentool anim key add promo.pen logo opacity --at 0 --value 0
pentool anim key add promo.pen logo opacity --at 0.5s --value 1
pentool anim key list promo.pen --node logo --json

# Inspect any instant without exporting.
pentool frame promo.pen --time 0.75s --out check.png
pentool tree promo.pen --time 0.75s --limit 20

# Export.
pentool export promo.pen frames/ --format png --fps 30
pentool export promo.pen promo.gif --fps 15 --scale 0.5 --dither ordered
pentool export promo.pen promo.mp4 --quality 23 --keyint 60
pentool export promo.pen promo.mp4 --bitrate 4M --from 1s --to 4s --dry-run
```

## Acceptance targets

- The same animated document exports byte-identical GIF and MP4 files on Linux,
  Windows, Intel macOS and Apple Silicon.
- Every MP4 plays in Chrome, Firefox, Safari, QuickTime, Windows Media Player
  and VLC, and on iOS and Android. Each check is recorded as release evidence.
- A 6-second 1080p30 motion-graphics clip exports to MP4 in under 10 seconds on
  the reference machine in `docs/performance.md`. The P-frame output is at
  least 5× smaller than all-intra at the same QP.
- A document with no timeline exports identically to v0.13.0.
- Migrating up and then down without animation is byte-exact. Migrating down
  with animation is refused without mutation.
- Cancelling at every export stage leaves no output file and no orphaned
  temporary file.
- The release binary grows by less than 1 MiB for all of v0.14.0's encoders.

## Explicitly deferred

- Audio import, mixing and encoding. These are v0.15.0. This release only
  reserves the track kinds and the MP4 track layout.
- Importing or decoding video files (MP4, MOV, WebM) as clips.
- Main or High profile H.264, CABAC, B-frames, interlacing, 10-bit and HDR
  video, HEVC, AV1, VP9 and WebM. These may follow if a documented need
  outweighs the binary-size cost.
- Expressions, scripting, physics, particle systems, and 3D cameras.
- Lottie, SMIL or CSS animation export. These need their own fidelity contract.
- Morphing between paths with different command structures.

## Acceptance demo

Open a two-page promo. Add a six-second timeline to the first page. Slide a logo
in with `ease-out`, fade a headline, animate a photo node's exposure, and pulse a
component instance's color. Scrub in the editor timeline and edit a key's easing
by dragging its curve. Export a 15 fps looping GIF and a 1080p30 MP4 from the
dialog, watching progress and cancelling once mid-export. Then export the same
files from the CLI on all four release targets and show they are byte-identical.
Play the MP4 in a browser, QuickTime and on a phone. Finally, show that the
second page, which has no timeline, still exports exactly as it did in v0.13.0.
