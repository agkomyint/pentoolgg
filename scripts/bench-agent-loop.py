#!/usr/bin/env python3
"""Time the agent loop: edit a document, then look at the result, ten times.

Compares a baseline Pentool binary (an agent runs an edit, then an export) with
a current one in three ways:

  edit+export   the same two commands as the baseline (render cache warm)
  --preview     one command per step: the edit returns the rendered page
  session       all ten steps through one `pentool session --jsonl` process

for a small document (docs/fixtures/v4-scene.pen) and a 55-layer document with
gradients, blurs and text that the script generates. Every timed image is
checked against the baseline's export of the same state, byte for byte.

    python scripts/bench-agent-loop.py --baseline OLD/pentool --pentool target/release/pentool

Prints a Markdown table of milliseconds per step (median of --runs runs).
"""

import argparse
import json
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STEPS = 10
SIZE = 1024


def call(binary, *args):
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True)
    if result.returncode != 0:
        sys.exit(f"{binary} {' '.join(map(str, args))} failed:\n{result.stderr.decode()}")
    return result.stdout


def large_document(path):
    """55 layers: a sky gradient, 18 blurred ellipses, 30 shapes, 6 text lines."""
    doc = json.loads(path.read_text(encoding="utf-8"))
    page = doc["pages"][0]
    page["canvas"].update({"width": 1200, "height": 675})
    layers = [page["layers"][0]]
    sky = {"kind": "fill", "id": "sky", "x": 0, "y": 0, "width": 1200, "height": 675,
           "fill": {"kind": "linear", "x1": 0, "y1": 0, "x2": 0, "y2": 675, "stops": [
               {"position": 0, "color": "#2f7fd6"}, {"position": 0.6, "color": "#8cc4ee"},
               {"position": 1, "color": "#f2e7c4"}]}}
    layers.insert(0, {"id": "sky", "name": "Sky", "locked": False, "visible": True,
                      "nodes": [sky]})
    for n in range(53):
        x, y = 40 + (n * 97) % 1100, 60 + (n * 53) % 560
        color = "#{:02x}{:02x}{:02x}".format(60 + n * 3, 140 - n, 90 + n * 2)
        if n < 18:
            node = {"kind": "ellipse", "id": f"cloud-{n}", "cx": x, "cy": y,
                    "radius_x": 90, "radius_y": 40, "opacity": 0.7,
                    "style": {"fill": {"fallback": "#ffffff"}},
                    "effects": [{"id": f"soft-{n}", "kind": "blur", "enabled": True,
                                 "opacity": 1, "blend_mode": "normal",
                                 "params": {"radius": 12 + n}}]}
        elif n < 47:
            node = {"kind": "rect", "id": f"shape-{n}", "x": x, "y": y, "width": 70,
                    "height": 46, "radius_x": 8, "radius_y": 8,
                    "style": {"fill": {"fallback": color}}}
        else:
            node = {"kind": "text", "id": f"label-{n}", "content": f"Layer {n}",
                    "font_family": "Atkinson Hyperlegible", "font_size": 28,
                    "font_weight": 700, "x": x, "y": y,
                    "style": {"fill": {"fallback": "#1b2430"}}}
        layers.insert(-1, {"id": f"layer-{n}", "name": f"Layer {n}", "locked": False,
                           "visible": True, "nodes": [node]})
    page["layers"] = layers
    path.write_text(json.dumps(doc, indent=2), encoding="utf-8")


def edit(doc, step):
    return ["shape", doc, "rect", f"agent-{step}", "--layer", "content",
            "--x", 20 + step * 24, "--y", 30 + step * 9, "--w", 40, "--h", 40,
            "--fill", "#e4572e"]


def fresh(source, folder, name):
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir(parents=True)
    doc = folder / name
    shutil.copy(source, doc)
    return doc


def loop(mode, binary, source, folder, scale):
    """Run the ten steps; return (seconds per step, last image bytes)."""
    doc = fresh(source, folder, "doc.pen")
    out = folder / "look.png"
    start = time.perf_counter()
    if mode == "session":
        lines = "".join(json.dumps([str(a) for a in edit(doc, step)
                                    + ["--no-focus", "--preview-size", SIZE,
                                       "--preview-out", out]]) + "\n"
                        for step in range(STEPS))
        result = subprocess.run([str(binary), "session", "--jsonl"], input=lines.encode(),
                                capture_output=True)
        replies = [json.loads(line) for line in result.stdout.splitlines()]
        if result.returncode != 0 or not all(r.get("ok") for r in replies):
            sys.exit(f"session failed: {result.stdout[-2000:]!r} {result.stderr[-2000:]!r}")
    else:
        for step in range(STEPS):
            if mode == "preview":
                call(binary, *edit(doc, step), "--no-focus", "--preview-size", SIZE,
                     "--preview-out", out)
            else:
                call(binary, *edit(doc, step))
                call(binary, "export", doc, out, "--scale", scale)
    elapsed = (time.perf_counter() - start) / STEPS
    return elapsed, out.read_bytes()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--pentool", required=True, type=Path)
    parser.add_argument("--runs", type=int, default=3)
    options = parser.parse_args()

    work = Path(tempfile.mkdtemp(prefix="pentool-agent-loop-"))
    try:
        small = work / "small.pen"
        shutil.copy(ROOT / "docs" / "fixtures" / "v4-scene.pen", small)
        call(options.pentool, "migrate", small, "--target", "6")
        large = work / "large.pen"
        shutil.copy(small, large)
        large_document(large)

        print(f"baseline: {call(options.baseline, '--version').decode().strip()}")
        print(f"current:  {call(options.pentool, '--version').decode().strip()}")
        print()
        print("| Document | Baseline edit+export | edit+export | --preview | session |")
        print("| --- | ---: | ---: | ---: | ---: |")
        for name, source in [("small", small), ("55 layers", large)]:
            # The preview's scale, so the baseline export draws the same pixels.
            probe = fresh(source, work / "probe", "doc.pen")
            result = json.loads(call(options.pentool, *edit(probe, 0), "--no-focus",
                                     "--preview-size", SIZE, "--preview-out",
                                     work / "probe.png"))
            scale = repr(result["preview"]["scale"])
            runs = {mode: [] for mode in ["baseline", "export", "preview", "session"]}
            for run in range(options.runs):
                images = {}
                for mode in runs:
                    binary = options.baseline if mode == "baseline" else options.pentool
                    seconds, images[mode] = loop(mode, binary, source,
                                                 work / f"{mode}-{run}", scale)
                    runs[mode].append(seconds * 1000)
                for mode, image in images.items():
                    if image != images["baseline"]:
                        sys.exit(f"{name}: {mode} image differs from the baseline export")
            cells = [f"{statistics.median(runs[m]):.0f} ms" for m in runs]
            print(f"| {name} | " + " | ".join(cells) + " |")
        print()
        print(f"Per step: one rect added, then the whole page at {SIZE} px. "
              f"Median of {options.runs} runs; every image byte-identical to the baseline.")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
