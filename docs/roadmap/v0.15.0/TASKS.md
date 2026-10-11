# v0.15.0 — Audio and complete multimedia documents

Give `.pen` timelines sound. Pages gain audio tracks: imported clips and
generated tones, with trimming, fades, keyframed gain and pan, and markers.
Pentool mixes them deterministically, previews them in the editor, and exports
them in sync with the video through its own encoders.

With v0.15.0, one `.pen` document can hold vector design, raster painting,
photography, motion and sound. It edits, versions, packages and exports all of
them from one offline Rust binary.

This is a minor release because it adds format data, commands and editor
features. Patch releases in the v0.14 line remain limited to bug fixes and
documentation.

## Product contract

- The Rust audio engine is authoritative. The CLI, the editor, the API and batch
  use one decoder, resampler, mixer and encoder. The browser plays a preview
  rendered by the Rust mixer; it never mixes independently.
- Audio and video share v0.14.0's rational time base. A sample position is
  exact; it is never rounded to a frame. Sync is exact by construction, not
  merely within tolerance.
- The mix is deterministic. The same document, sources and engine version
  produce the same PCM samples bit for bit on every release target, and the same
  encoded bytes. The mixer uses documented fixed-order arithmetic with no
  platform `libm` calls and no fused multiply-add.
- Sources are immutable and content-addressed, embedded or referenced by a
  verified relative path, like images and DNGs. A missing or changed source is
  reported, never silently replaced with silence.
- Audio edits are ordinary mutations. Dry run and commit share one planner, and
  failed, cancelled, stale or locked edits leave the document byte-for-byte
  unchanged.
- Decoding is bounded and hostile-input safe. Limits on duration, channel count,
  sample rate, decoded size and frame count are checked before work starts.
- Lean: decoders and encoders are Pentool's own Rust code or small audited pure
  Rust dependencies. No system codec libraries are linked or loaded at runtime.
  All of v0.15.0 adds less than 1 MiB to the release binary.
- Documents without audio, and v0.14.0 animated documents, render and export
  exactly as before.

## Audio model

```json
{"kind": "audio", "id": "music", "source": "sha256:…",
 "start": 0, "in": 1200, "out": 37200,
 "gain_db": -6, "pan": 0, "muted": false,
 "fade_in": 600, "fade_out": 1200, "fade_curve": "equal-power",
 "keys": {"gain_db": [{"t": 18000, "value": -6}, {"t": 19200, "value": -18,
                       "easing": "ease-in-out"}]}}
```

- An `audio` track places a clip on the page timeline. `start` is where it
  begins on the timeline; `in` and `out` trim the source. All three are in the
  timeline time base.
- `gain_db` and `pan` can be keyframed with the same easing set as v0.14.0.
  Interpolation is in decibels for gain and constant-power for pan.
- A `marker` track holds named, timed markers. These are used for beats,
  captions, chapters and snapping.
- The mix bus is fixed: 48 kHz, stereo, 32-bit float internally, with a
  documented true-peak limiter that can be switched off. Mono sources pan;
  multichannel sources fold down by a documented matrix.

## Must ship, in order

- [ ] **1. Freeze the audio specification and threat model.** Specify:
  - the audio track, clip and marker schemas;
  - exact time-base conversion, and resampling with its filter and how it is
    made deterministic;
  - gain, pan and fade math, and the mixer's summation order, limiter and
    dither;
  - the decoders and encoders with their profiles, bounds and malformed-input
    rules;
  - the MP4 audio track layout, A/V interleaving and sync, the loudness
    measurement method, the format version and its migration.

  Record the licensing position for each encoder (AAC-LC) as a release gate,
  as a decision by the project owner. Publish schemas and valid, invalid,
  adversarial and cross-platform fixtures before adding mutation commands.

- [ ] **2. Import audio sources.** `pentool audio add DOC ID --file F [--external]`
  imports a file with Pentool's own decoders:
  - WAV and AIFF: PCM, integer and float;
  - FLAC;
  - Ogg Vorbis;
  - MP3: patents expired; decoder only.

  Sources are content-addressed and immutable. `audio info` reports format,
  duration, rate, channels, peak and loudness. Unsupported or damaged input is
  refused with a precise error and leaves the document unchanged.

- [ ] **3. Add audio and marker tracks to the format.** Add the timeline tracks
  reserved in v0.14.0:
  - validation: clip bounds within the source, non-overlapping fades, keys,
    limits, and dangling sources;
  - migration in both directions: migrating down is refused while audio exists;
  - `anim track` and tree/search reporting of audio and markers, compact and
    paginated.

- [ ] **4. Build the deterministic mixer.** Implement `mix(page, range)`, which
  returns 48 kHz stereo float PCM:
  - fixed-order summation and a deterministic polyphase sinc resampler;
  - constant-power pan, gain and pan keys evaluated per sample by the v0.14.0
    easing math, and fades;
  - the optional limiter, and TPDF dither with a seeded generator when writing
    integer PCM.

  Add `pentool audio render DOC --out mix.wav [--from --to]`. Golden tests pin
  sample hashes for every feature.

- [ ] **5. Edit audio from the CLI and the agent API.**
  - `audio clip add|set|trim|split|move|remove`;
  - `audio key add|set|remove`;
  - `marker add|set|remove|list`;
  - `audio info`, plus reports of loudness, peak and the clipped-sample count
    for a time range.

  All edits run through the shared transaction path with dry run and
  `--if-revision`. Splitting a clip produces two clips whose concatenated mix
  is sample-identical to the original.

- [ ] **6. Write Pentool's own AAC-LC encoder.** Implement it in Rust:
  - MDCT with long and short windows, and a documented psychoacoustic model;
  - scalefactors, Huffman coding, and bit reservoir control;
  - constant-bitrate and quality modes.

  Encoding is deterministic and integer- or fixed-order-float only.
  Conformance: an independent decoder, used in CI only, decodes the output with
  a measured quality score on a reference corpus.

- [ ] **7. Export audio and video together in MP4.** Fill v0.14.0's reserved
  track layout:
  - an AAC-LC audio track (`mp4a`, `esds`) beside the H.264 video track;
  - deterministic chunk interleaving and `faststart`;
  - an edit list for AAC encoder priming, so audio starts exactly at time 0;
  - durations that agree to the sample.

  Also:
  - `export DOC out.mp4` includes audio automatically when the page has audible
    tracks, and `--no-audio` turns it off;
  - `--audio-bitrate` sets the AAC bitrate;
  - `export DOC out.m4a` exports audio only.

  A/V sync is verified frame-exactly in tests.

- [ ] **8. Export audio-only formats.** WAV (16- or 24-bit PCM, or 32-bit
  float), FLAC (Pentool's own encoder), and M4A (AAC-LC). `--normalize` targets
  a loudness level such as -14 LUFS, measured by the documented ITU-R BS.1770
  method, with a true-peak ceiling. GIF export ignores audio and says so in its
  report.

- [ ] **9. Add audio-driven animation.** `anim bind` drives any animatable
  property from a deterministic audio feature: an RMS envelope, the level of a
  frequency band, or onsets from a documented detector. The binding has
  smoothing, a range mapping and a clamp, and is either stored live or baked into
  ordinary keys. `marker detect --onsets` writes beat markers that keys snap to.
  Bindings are versioned and hashed, so replay never reinterprets them.

- [ ] **10. Build editor audio.** In the v0.14.0 timeline panel, add:
  - audio rows with waveforms, computed in Rust and cached as min/max peak
    pyramids;
  - clip drag, trim, split and fade handles, and gain and pan key lanes;
  - mute, solo and per-track meters;
  - marker rows, and snapping to markers, keys and frames.

  Playback streams the authoritative Rust mix to Web Audio, with the playhead
  and video preview locked to the audio clock. The export dialog gains audio
  options and loudness information. Every edit is one undoable change.

- [ ] **11. Add captions and chapters.** Markers with text become WebVTT and
  SRT sidecar export, MP4 chapter metadata, and optional burned-in captions
  rendered through the normal text path. Caption timing is checked for overlaps
  and minimum duration.

- [ ] **12. Integrate with history, diff, packages, components and privacy.**
  - Structural diff summarizes clip, key and marker changes.
  - Packages include audio sources, and verify reproduces the mix hash offline.
  - Components may contain audio clips, and instances may override gain and
    mute.
  - Audio metadata such as ID3, Vorbis comments and iXML is read, reported, and
    stripped by default on export, through the v0.11.0 privacy policy.
  - Batch, locks, page selection and unknown fields behave like other
    mutations.

- [ ] **13. Harden conformance, performance and failure recovery.**
  - Golden PCM hashes and byte-identical MP4, M4A and FLAC output on all four
    release targets.
  - Fuzzing of every decoder and of hostile durations, rates and channel counts.
  - Cancellation at every stage, disk-full handling, and transaction rollback.
  - Benchmarks of decoding, mixing and AAC encoding speed, recorded in
    `docs/performance.md`.
  - Release smoke tests that import WAV and FLAC and export an MP4 with sound.

- [ ] **14. Publish the multimedia guide and examples.** Add an end-to-end guide
  covering design, animation, audio, captions and export, for both people and
  agents. Add examples in `examples/`: a looping GIF, a 1080p promo with music
  and captions, and a beat-synced motion piece, each with expected outputs and
  hashes. Update the README to describe Pentool as a complete multimedia tool.

## CLI direction

Exact command names are provisional. They must reuse the shared transaction,
planner and structured-error conventions.

```sh
# Bring in sound.
pentool audio add promo.pen music --file ./track.flac
pentool audio add promo.pen vo --file ./voiceover.wav --external
pentool audio info promo.pen music --json

# Place, trim and shape it.
pentool audio clip add promo.pen music --start 0 --in 2s --out 62s --gain -6
pentool audio clip set promo.pen music --fade-in 1s --fade-out 2s
pentool audio key add promo.pen music gain_db --at 30s --value -18 --easing ease-in-out
pentool audio clip split promo.pen vo --at 12.5s

# Markers, beats and captions.
pentool marker detect promo.pen --track music --onsets --min-gap 0.25s
pentool marker add promo.pen --at 3s --text "Pentool 0.15" --kind caption
pentool anim bind promo.pen logo scale --from music --feature rms --range 1,1.15 --smooth 80ms

# Listen and export.
pentool audio render promo.pen --out check.wav --from 25s --to 35s
pentool export promo.pen promo.mp4 --quality 23 --audio-bitrate 192k --normalize -14
pentool export promo.pen promo.m4a
pentool export promo.pen captions.vtt
```

## Acceptance targets

- The same document gives identical PCM hashes and byte-identical MP4, M4A,
  WAV and FLAC files on Linux, Windows, Intel macOS and Apple Silicon.
- An exported MP4 with audio plays in sync in Chrome, Firefox, Safari,
  QuickTime, Windows Media Player and VLC, and on iOS and Android. A clap-sync
  test clip shows zero-frame offset at its start and after 10 minutes.
- Splitting, moving back and rejoining clips produces a sample-identical mix.
- Normalized exports measure within ±0.5 LU of target, with true peak at or
  below the ceiling.
- Documents without audio, and v0.14.0 animated documents, export exactly as
  before.
- Every decoder survives the fuzz corpus without panics, unbounded allocation
  or hangs.
- The release binary grows by less than 1 MiB for all of v0.15.0.
- A packaged multimedia document verifies and re-exports identically offline,
  with no network and no external tools.

## Explicitly deferred

- Importing or decoding video files (MP4, MOV, WebM) as timeline clips.
- Recording from a microphone, MIDI, synthesis beyond test tones, and plugin
  hosts (VST, AU, LV2).
- Multitrack mastering suites, surround or Atmos output, and time-stretching or
  pitch-shifting.
- Opus and Vorbis encoding, and WebM output.
- AI audio generation, voice cloning, transcription and stem separation. Any
  optional AI output must first be materialized through the v0.9 review and
  provenance workflow.
- Streaming, live broadcast, and network playback.

## Acceptance demo

Start from the v0.14.0 promo. Import a FLAC music bed and a WAV voice-over.
Trim the music, fade it in, and duck it under the voice with gain keys. Detect
onsets, snap the logo's scale pulses to the beats, and bind a glow to the music's
RMS envelope. Add caption markers for the voice-over. In the editor, scrub with
audio, solo the voice, adjust a fade by dragging, and watch the meters. Export a
1080p30 MP4 with captions normalized to -14 LUFS, plus an M4A, a WebVTT file and
a looping GIF. Play the MP4 in sync on desktop and on a phone. Show
byte-identical exports on all four release targets. Finally, package the
document, move it offline, verify it and re-export identically. One `.pen` file
holds design, painting, photography, motion and sound.
