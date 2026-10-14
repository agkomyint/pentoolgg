# Pentool v0.15 glass crystal

`glass-crystal.pen` is an editable version-10 scene rendered by Pentool's
deterministic static-3D engine. `glass-crystal.png` is its checked snapshot.

Try changing the scene:

```sh
pentool scene3d glass-crystal.pen camera crystal --azimuth 65 --elevation 18
pentool scene3d glass-crystal.pen material crystal set 0 --base-color '#80f4ffff' --roughness 0.08
pentool scene3d glass-crystal.pen environment crystal --sky '#161044ff' --ground '#02030aff' --intensity 0.8
pentool export glass-crystal.pen glass-crystal.png --renderer reference
```

The OBJ and MTL files are retained as readable, verified relative source assets.
Keep the three files together and the scene renders fully offline; `linked
collect` can produce a separate self-contained collected project when needed.
