# v0.19.0 — 3D animation

Animate 3D scenes on v0.14.0's timeline: keyframed cameras, lights and model
transforms, with deterministic rendering into PNG sequences, GIF and MP4.

## Product contract

- 3D animation reuses v0.14.0's time base, easing, tracks and encoders. There is
  no second timeline.
- Every frame is a pure function of the document and time, rendered by the
  reference renderer; the accelerator may preview but never defines export.
- Imported glTF animations (node transforms; skins as an explicit later step)
  map to ordinary keyframed tracks, never to opaque playback.
- Bounded: frame count, duration, triangle and texture budgets are checked
  before rendering starts; cancellation leaves no partial output.

## Must ship, in order

- [ ] **1. Specify 3D tracks:** camera, light and transform properties, easing
  and interpolation for rotations (quaternion slerp), with exact rational time.
- [ ] **2. Implement and validate** the tracks in the shared transaction path.
- [ ] **3. Import glTF node animations** as keyframes with an import report.
- [ ] **4. Deterministic frame renderer** and export through the v0.14.0
  encoders, with a frame cache keyed by content digest.
- [ ] **5. Editor:** timeline integration, camera path preview, scrubbing with
  progressive quality.
- [ ] **6. Evidence:** cross-platform frame hashes, budget benchmark, docs.

## Acceptance targets

- A turntable and a camera fly-through render identical frame hashes on every
  release target.
- Re-rendering one changed keyframe re-renders only affected frames.

## Explicitly deferred

- Skeletal skinning and rigging authoring, physics, particle systems, and
  cinematic effects such as depth of field and motion blur beyond the v0.14.0 set.
