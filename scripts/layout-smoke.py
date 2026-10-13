#!/usr/bin/env python3
"""Release smoke test for v0.14.0 layout: IDML import, compose, PDF/EPUB/IDML export.

usage: layout-smoke.py <pentool binary> <work dir>
"""
import json
import os
import subprocess
import sys
import zipfile

exe, work = sys.argv[1], sys.argv[2]
root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "docs", "fixtures", "idml", "minimal")
os.makedirs(work, exist_ok=True)


def run(*args):
    r = subprocess.run([exe, *args], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"FAILED: {' '.join(args)}\n{r.stdout}\n{r.stderr}")
    return r.stdout


# Pack the fixture; the mimetype entry must come first and be stored.
idml = os.path.join(work, "smoke.idml")
names = []
for base, _, files in os.walk(root):
    for f in files:
        full = os.path.join(base, f)
        names.append((os.path.relpath(full, root).replace(os.sep, "/"), full))
names.sort(key=lambda n: (n[0] != "mimetype", n[0]))
with zipfile.ZipFile(idml, "w") as z:
    for arc, full in names:
        z.write(full, arc, compress_type=zipfile.ZIP_STORED if arc == "mimetype" else zipfile.ZIP_DEFLATED)

doc = os.path.join(work, "smoke.pen")
run("idml-import", idml, doc, "--report", os.path.join(work, "report.json"))
report = json.loads(run("layout-preflight", doc))
print("preflight:", {k: report.get(k) for k in ("ok", "errors", "warnings") if k in report})
for fmt, out in (("pdf", "smoke.pdf"), ("epub", "smoke.epub")):
    path = os.path.join(work, out)
    run("layout-export", doc, path, "--format", fmt)
    assert os.path.getsize(path) > 100, out
back = os.path.join(work, "back.idml")
run("idml-export", doc, back)
assert zipfile.ZipFile(back).testzip() is None
run("idml-import", back, os.path.join(work, "again.pen"))
print("layout smoke ok")
