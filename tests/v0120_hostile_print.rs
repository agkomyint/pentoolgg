//! Hostile and malformed inputs to the print pipeline fail cleanly and boundedly.
use pentool::color::{bundle, catalog, pdfx, recipe};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/hostile-print.pen";

fn doc(width: u64, height: u64) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":width,"height":height,"background":"#ffffffff"});
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
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":0,"y":0,"width":width,"height":height}});
    raw
}

#[test]
fn oversized_pages_are_refused_before_any_render() {
    // 20000 x 20000 units at 300 ppi would be ~346M pixels; refuse up front.
    let raw = doc(20_000, 20_000);
    let started = std::time::Instant::now();
    let error = pdfx::build(&raw, Path::new(DOCUMENT), None, &pdfx::Options::default())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("limit-exceeded") || error.contains("canvas"),
        "{error}"
    );
    assert!(started.elapsed().as_secs() < 20, "refusal was not prompt");
}

#[test]
fn truncated_and_corrupted_pdf_structures_never_panic() {
    let mut raw = doc(40, 20);
    raw["pages"][0]["layers"][0]["nodes"] = json!([]);
    let built = pdfx::build(
        &raw,
        Path::new(DOCUMENT),
        None,
        &pdfx::Options {
            compress: true,
            ppi: 72.0,
            ..Default::default()
        },
    )
    .unwrap();
    pdfx::verify_structure(&built.bytes).unwrap();
    let bytes = built.bytes;
    // Every truncation at xref scale and a spread of earlier cuts must error.
    let mut cuts: Vec<usize> = (bytes.len().saturating_sub(400)..bytes.len()).collect();
    cuts.extend((0..bytes.len()).step_by(997));
    for cut in cuts {
        assert!(
            pdfx::verify_structure(&bytes[..cut]).is_err(),
            "truncation at {cut} was accepted"
        );
    }
    // Flipping one byte inside the xref offsets is detected, not trusted.
    let mut broken = bytes.clone();
    let at = bytes
        .windows(7)
        .rposition(|w| w == b"0000000")
        .expect("xref offsets present");
    broken[at + 3] = b'9';
    let _ = pdfx::verify_structure(&broken);
    assert!(pdfx::verify_structure(b"").is_err());
    assert!(pdfx::verify_structure(&[0xff; 4096]).is_err());
}

#[test]
fn hostile_recipes_and_manifests_are_rejected() {
    for text in [
        "",
        "null",
        "{",
        r#"{"version":1,"name":"../x"}"#,
        r#"{"version":1,"name":"a","ppi":1e999}"#,
        r#"{"version":1,"name":"a","min_ppi":-1}"#,
        r#"{"version":1,"name":"a","intent":"nope"}"#,
        r#"{"version":1,"name":"a","bleed_scale":"1"}"#,
    ] {
        assert!(recipe::parse(text.as_bytes()).is_err(), "accepted {text:?}");
    }
    let dir = std::env::temp_dir().join(format!("pentool-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for path in ["/etc/passwd", "a\\b", "C:/x", "a/../b", "", "a//b"] {
        std::fs::write(
            dir.join(bundle::MANIFEST),
            serde_json::to_vec(&json!({"version":1,"scope":"production-package",
                "files":[{"path":path,"bytes":0,"sha256":""}]}))
            .unwrap(),
        )
        .unwrap();
        assert!(
            bundle::verify(&dir).is_err(),
            "manifest path {path:?} was accepted"
        );
    }
    // An oversized manifest is bounded rather than parsed.
    std::fs::write(dir.join(bundle::MANIFEST), vec![b' '; 2 << 20]).unwrap();
    assert!(bundle::verify(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn import(icc: &[u8]) -> Result<Value, String> {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        icc,
        "press",
        "allowed",
        None,
        None,
    )
    .map(|_| raw)
    .map_err(|e| e.to_string())
}

#[test]
fn hostile_icc_bytes_never_panic_and_never_reach_output() {
    let good = include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc");
    assert!(import(good).is_ok());
    assert!(import(&[]).is_err());
    assert!(import(&[0u8; 128]).is_err());
    assert!(import(&[0xff; 4096]).is_err());
    // Truncations across the header, tag table and data must not panic.
    for cut in (0..good.len()).step_by((good.len() / 64).max(1)) {
        let _ = import(&good[..cut]);
    }
    // Corrupt single bytes in the header and tag table; outcomes may vary, panics may not.
    for at in (0..400.min(good.len())).step_by(37) {
        let mut bytes = good.to_vec();
        bytes[at] ^= 0xa5;
        if let Ok(raw) = import(&bytes) {
            let built = pdfx::build(
                &{
                    let mut r = raw;
                    r["pages"][0]["production"] = json!({"units_per_inch":72,
                        "trim":{"x":0,"y":0,"width":20,"height":10}});
                    r["pages"][0]["canvas"] =
                        json!({"width":20,"height":10,"background":"#ffffffff"});
                    r["pages"][0]["layers"][0]["nodes"] = json!([]);
                    r["color_management"]["roles"] = json!({"output":{
                        "destination":{"space":"icc-cmyk","profile":"press"},
                        "intent":"relative-colorimetric"}});
                    r
                },
                Path::new(DOCUMENT),
                None,
                &pdfx::Options {
                    compress: true,
                    ppi: 72.0,
                    ..Default::default()
                },
            );
            if let Ok(done) = built {
                pdfx::verify_structure(&done.bytes).unwrap();
            }
        }
    }
}

#[test]
fn rgb_profile_as_output_is_blocked_by_preflight_and_pdfx() {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    if let Err(error) = catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc"),
        "press",
        "allowed",
        None,
        None,
    ) {
        assert!(format!("{error:#}").contains("disagrees"), "{error:#}");
        return;
    }
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":0,"y":0,"width":20,"height":10}});
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    // Either the role is rejected outright or the report blocks production.
    if let Ok(report) = pentool::color::preflight::run(
        &raw,
        Path::new(DOCUMENT),
        None,
        &pentool::color::preflight::Options::default(),
    ) {
        assert_eq!(report["ok"], false);
    }
    assert!(pdfx::build(&raw, Path::new(DOCUMENT), None, &pdfx::Options::default()).is_err());
}

#[test]
fn failed_export_and_package_leave_no_artifacts_and_keep_existing_files() {
    let dir = std::env::temp_dir().join(format!("pentool-rollback-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // A page with no production geometry cannot produce PDF/X or a package.
    let mut raw = doc(40, 20);
    raw["pages"][0]
        .as_object_mut()
        .unwrap()
        .remove("production");
    let source = serde_json::to_vec(&raw).unwrap();
    let document = dir.join("doc.pen");
    std::fs::write(&document, &source).unwrap();

    let pdf = dir.join("out.pdf");
    assert!(pdfx::export(&raw, &document, None, &pdf, &pdfx::Options::default()).is_err());
    assert!(!pdf.exists(), "failed export left a file behind");

    let package = dir.join("pkg");
    assert!(bundle::build(&raw, &document, &source, None, &package, &package_options()).is_err());
    assert!(!package.exists(), "failed package left a directory behind");

    // An existing output is never replaced, even by a valid export.
    let valid = doc(40, 20);
    std::fs::write(&pdf, b"keep me").unwrap();
    assert!(pdfx::export(&valid, &document, None, &pdf, &pdfx::Options::default()).is_err());
    assert_eq!(std::fs::read(&pdf).unwrap(), b"keep me");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(package.join("marker"), b"x").unwrap();
    assert!(bundle::build(
        &valid,
        &document,
        &source,
        None,
        &package,
        &package_options()
    )
    .is_err());
    assert_eq!(std::fs::read(package.join("marker")).unwrap(), b"x");
    assert_eq!(std::fs::read(&document).unwrap(), source);
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("pentool-tmp") || n.contains("pentool-old"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn package_options() -> bundle::Options {
    bundle::Options {
        pdfx: pdfx::Options {
            compress: true,
            ppi: 72.0,
            ..Default::default()
        },
        preflight: pentool::color::preflight::Options::default(),
        separation_scale: 1.0,
        auto_trap: None,
        plate_bits: 8,
    }
}
