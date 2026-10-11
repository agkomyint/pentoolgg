//! PDF/X-4 flattened CMYK export: structure, boxes, determinism, refusals.
use pentool::color::{catalog, pdfx};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/pdfx.pen";

fn doc() -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"paint","x":0,"y":0,
        "width":200,"height":100,
        "fill":{"kind":"solid","color":{"space":"srgb","channels":[0.9,0.2,0.1]}}}]);
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":5});
    raw
}

fn options() -> pdfx::Options {
    pdfx::Options {
        compress: true,
        ppi: 72.0,
        created: Some("2026-10-10T12:30:00Z".into()),
        title: Some("Poster".into()),
        ..Default::default()
    }
}

fn contains(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn export_has_output_intent_boxes_metadata_and_is_deterministic() {
    let raw = doc();
    let before = raw.clone();
    let a = pdfx::build(&raw, Path::new(DOCUMENT), None, &options()).unwrap();
    let b = pdfx::build(&raw, Path::new(DOCUMENT), None, &options()).unwrap();
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(raw, before);
    let bytes = &a.bytes;
    assert!(bytes.starts_with(b"%PDF-1.6\n"));
    pdfx::verify_structure(bytes).unwrap();
    for needle in [
        "/S/GTS_PDFX",
        "/DestOutputProfile 8 0 R",
        "/GTS_PDFXVersion(PDF/X-4)",
        "<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>",
        "/Trapped/False",
        "/MediaBox[0 0 170 90]",
        "/BleedBox[0 0 170 90]",
        "/TrimBox[5 5 165 85]",
        "/ColorSpace[/ICCBased 8 0 R]",
        "/N 4/Alternate/DeviceCMYK",
        "<xmp:CreateDate>2026-10-10T12:30:00Z</xmp:CreateDate>",
        "/CreationDate(D:20261010123000Z)",
    ] {
        assert!(contains(bytes, needle), "missing {needle}");
    }
    assert_eq!(a.report["independent_validation"]["status"], "not-run");
    assert_eq!(a.report["output_intent"]["profile"], "press");
    assert_eq!(a.report["pixels"]["width"], 170);
    assert_eq!(a.report["pixels"]["height"], 90);
    assert_eq!(a.report["boxes"]["trim"], json!([5.0, 5.0, 165.0, 85.0]));
}

#[test]
fn refusals_are_named_and_leave_no_file() {
    let dir = std::env::temp_dir().join(format!("pentool-pdfx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.pdf");
    let cases: Vec<(Value, &str)> = {
        let mut cases = Vec::new();
        let mut no_geometry = doc();
        no_geometry["pages"][0]
            .as_object_mut()
            .unwrap()
            .remove("production");
        cases.push((no_geometry, "no production geometry"));
        let mut no_role = doc();
        no_role["color_management"]["roles"] = json!({});
        cases.push((no_role, "output color role"));
        let mut art = doc();
        art["pages"][0]["production"]["art"] = json!({"x":20,"y":10,"width":50,"height":50});
        cases.push((art, "TrimBox or ArtBox"));
        let mut overprint = doc();
        overprint["pages"][0]["layers"][0]["nodes"][0]["style"] =
            json!({"overprint":{"fill":true}});
        cases.push((overprint, "overprint"));
        cases
    };
    for (raw, needle) in cases {
        let error = pdfx::export(&raw, Path::new(DOCUMENT), None, &out, &options())
            .unwrap_err()
            .to_string();
        assert!(error.contains(needle), "{error}");
        assert!(!out.exists());
    }
    let error = pdfx::build(
        &doc(),
        Path::new(DOCUMENT),
        None,
        &pdfx::Options {
            compress: true,
            ppi: 5000.0,
            ..options()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("ppi"), "{error}");
    pdfx::export(&doc(), Path::new(DOCUMENT), None, &out, &options()).unwrap();
    let error = pdfx::export(&doc(), Path::new(DOCUMENT), None, &out, &options())
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compression_none_stores_raw_streams_and_reports_it() {
    let raw = doc();
    let flate = pdfx::build(&raw, Path::new(DOCUMENT), None, &options()).unwrap();
    let stored = pdfx::build(
        &raw,
        Path::new(DOCUMENT),
        None,
        &pdfx::Options {
            compress: false,
            ..options()
        },
    )
    .unwrap();
    pdfx::verify_structure(&stored.bytes).unwrap();
    assert!(contains(&flate.bytes, "/Filter/FlateDecode"));
    assert!(!contains(&stored.bytes, "/Filter/FlateDecode"));
    assert!(stored.bytes.len() > flate.bytes.len());
    assert_eq!(flate.report["compression"], "flate");
    assert_eq!(stored.report["compression"], "none");
}

/// The standard-library checker in `scripts/check-pdfx.py` shares no code with
/// Pentool. It is skipped where no Python interpreter is installed.
#[test]
fn independent_structure_checker_accepts_the_export() {
    let interpreter = ["python3", "python"].into_iter().find(|name| {
        std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    });
    let Some(interpreter) = interpreter else {
        eprintln!("skipped: no Python interpreter");
        return;
    };
    let built = pdfx::build(&doc(), Path::new(DOCUMENT), None, &options()).unwrap();
    let dir = std::env::temp_dir().join(format!("pentool-pdfx-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("out.pdf");
    std::fs::write(&file, &built.bytes).unwrap();
    let run = |path: &Path| {
        std::process::Command::new(interpreter)
            .args(["-I", "scripts/check-pdfx.py"])
            .arg(path)
            .output()
            .unwrap()
    };
    let output = run(&file);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{text}");
    assert!(text.contains("\"conformance_claim\": false"));
    // A corrupted TrimBox must be rejected.
    let needle = b"/TrimBox[5 5 165 85]";
    let at = built
        .bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap();
    let mut bad = built.bytes.clone();
    // Same length, so offsets stay valid and only the box nesting is wrong.
    bad[at..at + needle.len()].copy_from_slice(b"/TrimBox[5 5 175 85]");
    let bad_file = dir.join("bad.pdf");
    std::fs::write(&bad_file, &bad).unwrap();
    assert!(!run(&bad_file).status.success());
    let _ = std::fs::remove_dir_all(&dir);
}
