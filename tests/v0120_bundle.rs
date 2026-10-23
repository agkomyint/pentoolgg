//! Production packages: contents, digest verification and no partial output.
use pentool::color::{bundle, catalog, pdfx, preflight};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pentool-bundle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn doc(path: &Path) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    catalog::import(
        &mut raw,
        path,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"paint","x":0,"y":0,
        "width":200,"height":100,
        "style":{"fill":{"space":"icc-cmyk","profile":"press","channels":[0.0,0.8,0.9,0.0]}}}]);
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":9});
    raw
}

fn options() -> bundle::Options {
    bundle::Options {
        pdfx: pdfx::Options {
            compress: true,
            ppi: 72.0,
            created: Some("2026-10-10T00:00:00Z".into()),
            ..Default::default()
        },
        preflight: preflight::Options::default(),
        separation_scale: 1.0,
        auto_trap: None,
        plate_bits: 8,
    }
}

#[test]
fn package_contains_every_dependency_and_verifies_offline() {
    let dir = temp("ok");
    let document = dir.join("a.pen");
    let raw = doc(&document);
    let source = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&document, &source).unwrap();
    let out = dir.join("pkg");
    let manifest = bundle::build(&raw, &document, &source, None, &out, &options()).unwrap();
    assert_eq!(manifest["plates"]["status"], "written");
    assert_eq!(manifest["independent_validation"]["status"], "not-run");
    let paths: Vec<&str> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    for needle in [
        "document.pen",
        "output.pdf",
        "preflight.json",
        "plates/separations.json",
        "proof.png",
        "ink-map.json",
    ] {
        assert!(paths.contains(&needle), "{needle} missing from {paths:?}");
    }
    assert!(paths
        .iter()
        .any(|p| p.starts_with("profiles/") && p.ends_with(".icc")));
    assert_eq!(manifest["dependencies"][0]["reference"], "press");
    assert_eq!(
        manifest["proof"]["scope"], "soft-proof-image",
        "{}",
        manifest["proof"]
    );
    assert_eq!(manifest["proof"]["paper_simulation"], false);
    let proof = std::fs::read(out.join("proof.png")).unwrap();
    assert_eq!(&proof[1..4], b"PNG");
    assert_eq!(std::fs::read(out.join("document.pen")).unwrap(), source);

    let ok = bundle::verify(&out).unwrap();
    assert_eq!(ok["ok"], true, "{ok}");

    // A file smuggled in beside the manifest is reported, not ignored.
    std::fs::write(out.join("extra-evil.txt"), b"smuggled").unwrap();
    let extra = bundle::verify(&out).unwrap();
    assert_eq!(extra["ok"], false, "{extra}");
    assert_eq!(extra["problems"][0]["path"], "extra-evil.txt");
    assert_eq!(extra["problems"][0]["problem"], "unlisted");
    std::fs::remove_file(out.join("extra-evil.txt")).unwrap();

    // Tampering with any packaged file is detected.
    std::fs::write(out.join("output.pdf"), b"tampered").unwrap();
    let bad = bundle::verify(&out).unwrap();
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["problems"][0]["path"], "output.pdf");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn preflight_errors_and_existing_targets_leave_no_package() {
    let dir = temp("refuse");
    let document = dir.join("a.pen");
    let mut raw = doc(&document);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(100);
    raw["pages"][0]["canvas"]["background"] = json!("#00000000");
    let source = serde_json::to_vec(&raw).unwrap();
    let out = dir.join("pkg");
    let error = bundle::build(&raw, &document, &source, None, &out, &options())
        .unwrap_err()
        .to_string();
    assert!(error.contains("bleed-incomplete"), "{error}");
    assert!(!out.exists());
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        leftovers.iter().all(|n| !n.contains("pentool-tmp")),
        "{leftovers:?}"
    );

    std::fs::create_dir_all(&out).unwrap();
    let error = bundle::build(&doc(&document), &document, &source, None, &out, &options())
        .unwrap_err()
        .to_string();
    assert!(error.contains("already exists"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn verification_rejects_unsafe_manifest_paths() {
    let dir = temp("unsafe");
    std::fs::write(
        dir.join(bundle::MANIFEST),
        serde_json::to_vec(&json!({"version":1,"scope":"production-package",
            "files":[{"path":"../escape","bytes":0,"sha256":""}]}))
        .unwrap(),
    )
    .unwrap();
    let error = bundle::verify(&dir).unwrap_err().to_string();
    assert!(error.contains("unsafe package path"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn linked_images_travel_at_their_relative_paths_and_verify() {
    let dir = temp("linked");
    let document = dir.join("a.pen");
    let mut raw = doc(&document);
    let mut png = Vec::new();
    image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::create_dir_all(dir.join("art")).unwrap();
    std::fs::write(dir.join("art/photo.png"), &png).unwrap();
    pentool::image::add(
        &mut raw,
        None,
        "art",
        "photo",
        &png,
        pentool::image::external_storage(Path::new("art/photo.png")).unwrap(),
        10.0,
        10.0,
        20.0,
        20.0,
        pentool::image::Fit::Fill,
    )
    .unwrap();
    let source = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&document, &source).unwrap();
    let out = dir.join("pkg");
    let manifest = bundle::build(&raw, &document, &source, None, &out, &options()).unwrap();
    assert_eq!(manifest["assets"]["linked_images"], 1);
    assert_eq!(std::fs::read(out.join("art/photo.png")).unwrap(), png);
    assert!(manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "art/photo.png" && f["role"] == "image-asset"));
    let report = bundle::verify(&out).unwrap();
    assert_eq!(report["ok"], true, "{report}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn package_plates_follow_the_recipe_depth_and_trap_options() {
    let dir = temp("depth");
    let document = dir.join("a.pen");
    let raw = doc(&document);
    let source = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&document, &source).unwrap();
    let mut custom = options();
    custom.plate_bits = 16;
    custom.auto_trap = Some(0.5);
    let out = dir.join("pkg");
    let manifest = bundle::build(&raw, &document, &source, None, &out, &custom).unwrap();
    assert_eq!(manifest["plates"]["status"], "written", "{manifest}");
    let plate = std::fs::read(out.join("plates/plate-01-M.png")).unwrap();
    // PNG IHDR bit depth byte follows width, height and sits at offset 24.
    assert_eq!(plate[24], 16, "plates are 16-bit grayscale");
    let report: Value =
        serde_json::from_slice(&std::fs::read(out.join("plates/separations.json")).unwrap())
            .unwrap();
    assert_eq!(report["bits"], 16, "{report}");
    assert_eq!(bundle::verify(&out).unwrap()["ok"], true);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Packages of identical bytes built at the same time must not share a staging
/// directory (a hosted-CI Linux run lost a plate file to exactly that race).
#[test]
fn concurrent_packages_of_the_same_document_do_not_collide() {
    let dir = temp("concurrent");
    let document = dir.join("a.pen");
    let raw = doc(&document);
    let source = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&document, &source).unwrap();
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|index| {
                let (raw, document, source) = (&raw, &document, &source);
                let out = dir.join(format!("pkg-{index}"));
                scope.spawn(move || {
                    bundle::build(raw, document, source, None, &out, &options()).unwrap()
                })
            })
            .collect();
        for job in jobs {
            assert_eq!(job.join().unwrap()["plates"]["status"], "written");
        }
    });
}
