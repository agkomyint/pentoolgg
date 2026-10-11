"""Independent structural checks of a Pentool PDF/X-4 draft.

Development-only and standard library only: it re-parses the PDF bytes without
using any Pentool code, so a mistake in `pdfx::verify_structure` cannot hide here.
It is NOT a PDF/X conformance validator (no preflight engine, no font or
transparency rules); passing it is evidence for the structural checks listed in
CHECKS only. Usage: python -I scripts/check-pdfx.py FILE.pdf [FILE.pdf ...]
Exit status is 0 only when every file passes every check.
"""
import json
import re
import sys
import zlib

CHECKS = [
    "header", "eof", "xref-offsets", "trailer-id", "catalog-output-intent",
    "icc-profile-stream", "info-pdfx-version", "xmp-matches-info", "boxes-nested",
    "stream-lengths", "image-sample-count", "no-forbidden-keys",
]

OBJ = re.compile(rb"(\d+) 0 obj\n(.*?)\nendobj\n", re.S)


def objects(data):
    out = {}
    for match in OBJ.finditer(data):
        out[int(match.group(1))] = match.group(2)
    return out


def split_stream(body):
    marker = body.find(b"stream\n")
    if marker < 0:
        return body, None
    dictionary = body[:marker]
    payload = body[marker + 7:]
    end = payload.rfind(b"\nendstream")
    return dictionary, payload[:end] if end >= 0 else payload


def ref(text, key):
    found = re.search(rb"/" + key + rb"\s+(\d+) 0 R", text)
    return int(found.group(1)) if found else None


def boxes(page):
    result = {}
    for name in (b"MediaBox", b"BleedBox", b"TrimBox", b"ArtBox", b"CropBox"):
        found = re.search(rb"/" + name + rb"\[([^\]]*)\]", page)
        if found:
            result[name.decode()] = [float(v) for v in found.group(1).split()]
    return result


def contains(outer, inner):
    return (outer[0] <= inner[0] and outer[1] <= inner[1]
            and outer[2] >= inner[2] and outer[3] >= inner[3])


def check(path):
    data = open(path, "rb").read()
    results = {name: None for name in CHECKS}
    notes = []

    def record(name, ok, why=""):
        results[name] = bool(ok)
        if not ok:
            notes.append(name + ": " + why)

    record("header", data.startswith(b"%PDF-1.") and data[8:9] == b"\n"
           and data[9:10] == b"%" and max(data[10:14]) > 127,
           "needs %PDF-1.x and a binary comment line")
    record("eof", data.endswith(b"%%EOF\n"), "missing %%EOF")
    start = re.search(rb"startxref\n(\d+)\n%%EOF\n$", data)
    xref_at = int(start.group(1)) if start else -1
    table = None
    if xref_at >= 0:
        table = re.match(
            rb"xref\n0 (\d+)\n((?:\d{10} \d{5} [nf] ?\n)+)trailer\n(.*?)\nstartxref",
            data[xref_at:], re.S)
    if not table:
        record("xref-offsets", False, "startxref does not point at a parseable xref table")
        record("trailer-id", False, "no trailer")
        return results, notes
    count = int(table.group(1))
    entries = table.group(2).decode().splitlines()
    ok = len(entries) == count
    for index, line in enumerate(entries[1:], start=1):
        offset = int(line[:10])
        marker = ("%d 0 obj" % index).encode()
        ok = ok and data[offset:offset + len(marker)] == marker
    record("xref-offsets", ok, "an xref offset does not land on its object")
    trailer = table.group(3)
    ids = re.search(rb"/ID\[<([0-9a-fA-F]{32})><([0-9a-fA-F]{32})>\]", trailer)
    record("trailer-id", ids and ref(trailer, b"Root") and ref(trailer, b"Info")
           and re.search(rb"/Size %d" % count, trailer),
           "trailer needs Root, Info, Size and two 16-byte IDs")

    objs = objects(data)
    root = objs.get(ref(trailer, b"Root") or -1, b"")
    intents = re.search(rb"/OutputIntents\[(\d+) 0 R\]", root)
    intent = objs.get(int(intents.group(1)), b"") if intents else b""
    record("catalog-output-intent",
           b"/Type/Catalog" in root and b"/S/GTS_PDFX" in intent
           and b"/OutputConditionIdentifier(" in intent and ref(intent, b"DestOutputProfile"),
           "catalog needs one GTS_PDFX output intent with an identifier and profile")

    profile_dict, profile = split_stream(objs.get(ref(intent, b"DestOutputProfile") or -1, b""))
    icc_ok = False
    if profile is not None:
        raw = zlib.decompress(profile) if b"/FlateDecode" in profile_dict else profile
        icc_ok = (b"/N 4" in profile_dict and len(raw) >= 128 and raw[36:40] == b"acsp"
                  and raw[16:20] == b"CMYK" and int.from_bytes(raw[:4], "big") == len(raw))
    record("icc-profile-stream", icc_ok,
           "DestOutputProfile must be a 4-component CMYK ICC stream with a consistent header")

    info = objs.get(ref(trailer, b"Info") or -1, b"")
    version = re.search(rb"/GTS_PDFXVersion\(([^)]*)\)", info)
    record("info-pdfx-version", version and version.group(1).startswith(b"PDF/X-")
           and re.search(rb"/Trapped/(True|False)", info) and b"/CreationDate(" in info,
           "Info needs GTS_PDFXVersion, Trapped and CreationDate")
    meta = ref(root, b"Metadata")
    _, xmp = split_stream(objs.get(meta or -1, b""))
    xmp_version = re.search(rb"<pdfxid:GTS_PDFXVersion>([^<]*)<", xmp or b"")
    record("xmp-matches-info", version and xmp_version
           and xmp_version.group(1) == version.group(1),
           "XMP pdfxid:GTS_PDFXVersion must equal the Info value")

    page = next((body for body in objs.values() if b"/Type/Page/" in body), b"")
    found = boxes(page)
    record("boxes-nested", "MediaBox" in found and "TrimBox" in found
           and len(found.get("MediaBox", [])) == 4
           and contains(found["MediaBox"], found.get("BleedBox", found["MediaBox"]))
           and contains(found.get("BleedBox", found["MediaBox"]), found["TrimBox"]),
           "MediaBox contains BleedBox contains TrimBox; TrimBox required")

    lengths_ok = True
    for number, body in objs.items():
        dictionary, payload = split_stream(body)
        if payload is None:
            continue
        declared = re.search(rb"/Length (\d+)", dictionary)
        lengths_ok = lengths_ok and declared and int(declared.group(1)) == len(payload)
        if b"/FlateDecode" in dictionary:
            try:
                zlib.decompress(payload)
            except zlib.error:
                lengths_ok = False
                notes.append("object %d: Flate stream does not decode" % number)
    record("stream-lengths", lengths_ok, "a stream /Length is wrong or its Flate data is corrupt")

    samples_ok = True
    for body in objs.values():
        dictionary, payload = split_stream(body)
        if payload is None or b"/Subtype/Image" not in dictionary:
            continue
        width = int(re.search(rb"/Width (\d+)", dictionary).group(1))
        height = int(re.search(rb"/Height (\d+)", dictionary).group(1))
        raw = zlib.decompress(payload) if b"/FlateDecode" in dictionary else payload
        samples_ok = samples_ok and len(raw) == width * height * 4
    record("image-sample-count", samples_ok, "image data must be width*height*4 CMYK bytes")

    forbidden = [k.decode() for k in (b"/Encrypt", b"/JavaScript", b"/Launch", b"/DeviceRGB",
                                      b"/DeviceGray", b"/EmbeddedFile") if k in data]
    record("no-forbidden-keys", not forbidden, "forbidden keys present: %s" % forbidden)
    return results, notes


def main(paths):
    ok = bool(paths)
    reports = []
    for path in paths:
        results, notes = check(path)
        passed = all(v is True for v in results.values())
        ok = ok and passed
        reports.append({"file": path, "passed": passed, "checks": results, "failures": notes})
    print(json.dumps({"scope": "pentool-pdfx-structure-check", "conformance_claim": False,
                      "ok": ok, "reports": reports}, indent=2))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
