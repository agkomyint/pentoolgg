//! Bounded RAW ingestion (v0.11.0 item 3): the DNG conformance fixtures,
//! recorded asset facts and stage-1 decoding.
use pentool::photo::dng::Dng;
use pentool::photo::raw::{self, Decode, Demosaic, Highlights};
use serde_json::{json, Value};
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new("docs/fixtures").join(name)).unwrap()
}

#[test]
fn raw_fixtures_inspect_or_fail_with_their_manifest_codes() {
    let manifest: Value = serde_json::from_slice(&fixture("photo-conformance.json")).unwrap();
    let mut checked = 0;
    for entry in manifest["fixtures"].as_array().unwrap() {
        let name = entry["file"].as_str().unwrap();
        if !(name.ends_with(".dng") || name.ends_with(".cr2")) {
            continue;
        }
        let bytes = fixture(name);
        let result = Dng::inspect(&bytes).and_then(|dng| {
            raw::decode(&dng, &Decode::as_shot(&dng))?;
            Ok(())
        });
        match entry["expect"].as_str().unwrap() {
            "ok" => result.unwrap_or_else(|error| panic!("{name}: {error}")),
            _ => {
                let error = result.expect_err(name).to_string();
                let code = format!("[{}]", entry["code"].as_str().unwrap());
                assert!(error.starts_with(&code), "{name}: {error}");
                if name.ends_with(".cr2") {
                    assert!(error.contains("convert to DNG"), "{error}");
                }
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn rggb_fixture_records_capture_and_raw_facts_without_private_fields() {
    let bytes = fixture("photo-dng-rggb16.dng");
    let facts = Dng::inspect(&bytes).unwrap().asset_facts();
    assert_eq!(
        facts,
        json!({
            "media_type": "image/x-adobe-dng",
            "kind": "raw",
            "byte_length": 1458,
            "pixel_width": 12,
            "pixel_height": 12,
            "orientation": 1,
            "bit_depth": 16,
            "input_profile": "camera",
            "capture": {
                "make": "Pentool",
                "model": "Synthetic Conformance Body",
                "lens": "Synthetic 35mm F2.8",
                "focal_length": 35,
                "aperture": 2.8,
                "exposure_time": [1, 125],
                "iso": 400,
                "flash": false,
                "captured": "2026-05-17T19:42:08+02:00"
            },
            "raw": {
                "dng_version": "1.6.0.0",
                "layout": "cfa",
                "cfa_pattern": "RGGB",
                "unique_camera_model": "Pentool Synthetic Conformance Body",
                "as_shot_neutral": [0.5, 1, 0.75],
                "baseline_exposure": 0,
                "opcodes": []
            }
        })
    );
    // The fixture carries GPS, a serial number and an owner; none is recorded.
    let text = facts.to_string();
    for private in ["gps", "serial", "owner", "Latitude"] {
        assert!(
            !text.to_lowercase().contains(&private.to_lowercase()),
            "{private}"
        );
    }
}

#[test]
fn rggb_fixture_decodes_its_known_mosaic() {
    let bytes = fixture("photo-dng-rggb16.dng");
    let dng = Dng::inspect(&bytes).unwrap();
    let options = Decode {
        demosaic: Demosaic::Bilinear,
        highlights: Highlights::Clip,
        neutral: [1.0; 3],
    };
    let rgb = raw::decode(&dng, &options).unwrap();
    assert_eq!((rgb.width, rgb.height), (12, 12));
    // Output (x, y) is stored (x + 2, y + 2); its own CFA color is the stored value
    // normalized between black 256 and white 4095.
    for y in 0..12 {
        for x in 0..12 {
            let (sx, sy) = (x + 2, y + 2);
            let stored = 256 + (sx * 211 + sy * 97) % 3600;
            let color = [[0, 1], [1, 2]][sy % 2][sx % 2];
            let expected = ((stored - 256) as f64 / (4095.0 - 256.0)) as f32;
            assert_eq!(rgb.rgb[(y * 12 + x) * 3 + color], expected, "({x}, {y})");
        }
    }
    let mhc = raw::decode(&dng, &Decode::as_shot(&dng)).unwrap();
    assert!(mhc.rgb.iter().all(|v| v.is_finite() && *v >= 0.0));
    assert_eq!(mhc, raw::decode(&dng, &Decode::as_shot(&dng)).unwrap());
}

#[test]
fn linear_fixture_records_samples_and_decodes_without_demosaic() {
    let bytes = fixture("photo-dng-linear16.dng");
    let dng = Dng::inspect(&bytes).unwrap();
    let facts = dng.asset_facts();
    assert_eq!(facts["byte_length"], 856);
    assert_eq!(
        facts["capture"],
        json!({"make": "Pentool", "model": "Synthetic Conformance Body"})
    );
    assert_eq!(facts["raw"]["layout"], "linear-raw");
    assert_eq!(facts["raw"]["cfa_pattern"], Value::Null);
    assert_eq!(facts["raw"]["samples"], 3);
    let options = Decode {
        demosaic: Demosaic::Mhc,
        highlights: Highlights::Clip,
        neutral: [1.0; 3],
    };
    let rgb = raw::decode(&dng, &options).unwrap();
    assert_eq!((rgb.width, rgb.height), (8, 8));
    let (black, white) = (dng.black[0], dng.white[0]);
    for y in 0..8 {
        for x in 0..8 {
            for c in 0..3 {
                let stored = 256 + (x * 467 + y * 331 + c * 1201) % 3800;
                let expected = ((stored as f64 - black) / (white - black)).min(1.0) as f32;
                assert_eq!(rgb.rgb[(y * 8 + x) * 3 + c], expected);
            }
        }
    }
}

struct Workspace(std::path::PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-raw-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Workspace(root)
    }

    fn document(&self) -> std::path::PathBuf {
        let path = self.0.join("catalog.pen");
        let raw = pentool::scene::new_document(40, 30);
        std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        path
    }

    fn run(&self, args: &[&str]) -> (bool, Value, String) {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap();
        let stdout = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        (
            output.status.success(),
            stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn raw_add_upgrades_to_v7_with_explicit_import_defaults() {
    let work = Workspace::new("add");
    let document = work.document();
    std::fs::write(work.0.join("capture.dng"), fixture("photo-dng-rggb16.dng")).unwrap();
    let before = std::fs::read(&document).unwrap();
    let (ok, dry, error) = work.run(&[
        "raw",
        "add",
        "catalog.pen",
        "hero",
        "--file",
        "capture.dng",
        "--dry-run",
    ]);
    assert!(ok, "{error}");
    assert_eq!(dry["change"]["dry_run"], true);
    assert_eq!(std::fs::read(&document).unwrap(), before);

    let (ok, added, error) =
        work.run(&["raw", "add", "catalog.pen", "hero", "--file", "capture.dng"]);
    assert!(ok, "{error}");
    assert_eq!(added["result"], dry["result"]);
    assert_eq!(added["result"]["upgraded"], true);
    let raw = read(&document);
    assert_eq!(raw["version"], 7);
    assert_eq!(raw["compositing"]["color_space"], "srgb8");
    let catalog = &raw["photography"];
    let digest = "sha256:2d0adb68a1676d0347a4a7346e892fabfeadb4c6ec74c2ab1709c0aaa254079b";
    let asset = &catalog["assets"][digest];
    assert_eq!(asset["storage"]["kind"], "embedded");
    let mut facts = asset.clone();
    facts.as_object_mut().unwrap().remove("storage");
    let bytes = fixture("photo-dng-rggb16.dng");
    assert_eq!(facts, Dng::inspect(&bytes).unwrap().asset_facts());
    let photo = &catalog["photos"][0];
    assert_eq!(photo["id"], "hero");
    assert_eq!(photo["source"], digest);
    assert_eq!(photo["name"], "capture");
    assert_eq!(
        photo["variants"],
        json!([{"id": "master", "name": "Master", "develop": {
            "process": 1,
            "raw": {"demosaic": "mhc", "highlights": "blend", "camera_profile": "embedded"},
            "white_balance": {"mode": "as-shot"},
            "lens": {"profile": "embedded-opcodes"},
            "detail": {"sharpening": {"amount": 40, "radius": 1, "detail": 25}, "noise": {"color": 25}}
        }}])
    );

    // The same bytes, stored externally, deduplicate onto the first record.
    let (ok, again, error) = work.run(&[
        "raw",
        "add",
        "catalog.pen",
        "second",
        "--file",
        "capture.dng",
        "--external",
        "--camera-profile",
        "matrix-only",
    ]);
    assert!(ok, "{error}");
    assert_eq!(again["result"]["deduplicated"], true);
    assert_eq!(again["result"]["upgraded"], false);
    let raw = read(&document);
    assert_eq!(raw["photography"]["assets"].as_object().unwrap().len(), 1);
    assert_eq!(
        raw["photography"]["photos"][1]["variants"][0]["develop"]["raw"]["camera_profile"],
        "matrix-only"
    );

    let (ok, info, error) = work.run(&["raw", "info", "catalog.pen", "hero"]);
    assert!(ok, "{error}");
    assert_eq!(info["asset"], digest);
    assert_eq!(info["storage"], json!({"kind": "embedded"}));
    assert_eq!(info["source"], facts);
    assert_eq!(
        info["variants"],
        json!([{"id": "master", "name": "Master"}])
    );
    assert!(!info.to_string().contains("\"data\""));
    let (ok, _, error) = work.run(&["raw", "info", "catalog.pen", "missing"]);
    assert!(!ok && error.contains("[missing-resource]"), "{error}");
}

#[test]
fn raw_add_stores_an_external_source_by_relative_path() {
    let work = Workspace::new("external");
    let document = work.document();
    std::fs::create_dir_all(work.0.join("raw")).unwrap();
    std::fs::write(
        work.0.join("raw/capture.dng"),
        fixture("photo-dng-linear16.dng"),
    )
    .unwrap();
    let (ok, _, error) = work.run(&[
        "raw",
        "add",
        "catalog.pen",
        "detail",
        "--file",
        "raw/capture.dng",
        "--external",
    ]);
    assert!(ok, "{error}");
    let (ok, info, error) = work.run(&["raw", "info", "catalog.pen", "detail"]);
    assert!(ok, "{error}");
    assert_eq!(
        info["storage"],
        json!({"kind": "external", "path": "raw/capture.dng"})
    );
    assert!(!std::fs::read_to_string(&document)
        .unwrap()
        .contains("\"data\""));
}

#[test]
fn raw_add_refuses_bad_input_without_mutation() {
    let work = Workspace::new("refuse");
    let document = work.document();
    std::fs::write(work.0.join("capture.dng"), fixture("photo-dng-rggb16.dng")).unwrap();
    std::fs::write(work.0.join("old.cr2"), fixture("photo-raw-cr2-header.cr2")).unwrap();
    std::fs::write(work.0.join("xtrans.dng"), fixture("photo-dng-xtrans.dng")).unwrap();
    let outside = Workspace::new("outside");
    std::fs::write(
        outside.0.join("capture.dng"),
        fixture("photo-dng-rggb16.dng"),
    )
    .unwrap();
    let outside_path = format!(
        "../{}/capture.dng",
        outside.0.file_name().unwrap().to_string_lossy()
    );
    let unknown_profile = format!("sha256:{}", "0".repeat(64));
    let before = std::fs::read(&document).unwrap();
    for (args, code) in [
        (vec!["--file", "old.cr2"], "[unsupported-capability]"),
        (vec!["--file", "xtrans.dng"], "[unsupported-capability]"),
        (vec!["--file", "missing.dng"], "[missing-resource]"),
        (vec!["--file", &outside_path, "--external"], "[unsafe-path]"),
        (
            vec!["--file", "capture.dng", "--camera-profile", "neutral"],
            "[invalid-develop]",
        ),
        (
            vec![
                "--file",
                "capture.dng",
                "--camera-profile",
                &unknown_profile,
            ],
            "[missing-resource]",
        ),
        (vec!["--file", "capture.dng", "--if-revision", "stale"], ""),
    ] {
        let mut command = vec!["raw", "add", "catalog.pen", "hero"];
        command.extend(args.iter().copied());
        let (ok, _, error) = work.run(&command);
        assert!(!ok, "{command:?} succeeded");
        assert!(error.contains(code), "{command:?}: {error}");
        assert_eq!(std::fs::read(&document).unwrap(), before, "{command:?}");
    }
    let (ok, _, error) = work.run(&["raw", "add", "catalog.pen", "a/b", "--file", "capture.dng"]);
    assert!(!ok && error.contains("[malformed-resource]"), "{error}");
    assert_eq!(std::fs::read(&document).unwrap(), before);
    assert!(!work.0.join(".pentool").exists());
}

#[test]
fn v7_migration_round_trips_only_without_photo_content() {
    let work = Workspace::new("migrate");
    let document = work.document();
    let (ok, _, error) = work.run(&["migrate", "catalog.pen", "--target", "7"]);
    assert!(ok, "{error}");
    let raw = read(&document);
    assert_eq!(raw["version"], 7);
    assert!(raw.get("photography").is_none());
    let (ok, _, error) = work.run(&["migrate", "catalog.pen", "--target", "6"]);
    assert!(ok, "{error}");
    assert_eq!(read(&document)["version"], 6);

    // An empty catalog is representable by version 6; it is removed.
    let mut raw = read(&document);
    raw["version"] = json!(7);
    raw["photography"] =
        json!({"engine": 1, "working_space": "prophoto-linear", "assets": {}, "photos": []});
    pentool::transaction::validate_value(&raw).unwrap();
    let downgraded = pentool::composite::migrate(raw.clone()).unwrap();
    assert_eq!(downgraded["version"], 6);
    assert!(downgraded.get("photography").is_none());

    // Version 6 cannot carry the catalog at all.
    let mut v6 = raw.clone();
    v6["version"] = json!(6);
    let error = pentool::transaction::validate_value(&v6)
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("[unsupported-capability]"), "{error}");
}

type Tamper = Box<dyn Fn(&mut Value)>;

#[test]
fn v7_catalog_rejects_tampered_or_inconsistent_records() {
    use base64::Engine;
    let base = serde_json::from_slice::<Value>(&fixture("photo-v7.pen")).unwrap();
    pentool::transaction::validate_value(&base).unwrap();
    const DIGEST: &str = "sha256:2d0adb68a1676d0347a4a7346e892fabfeadb4c6ec74c2ab1709c0aaa254079b";
    let cases: Vec<(&str, Tamper)> = vec![
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["assets"][DIGEST]["pixel_width"] = json!(16)),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["assets"][DIGEST]["capture"]["iso"] = json!(800)),
        ),
        (
            "[hash-mismatch]",
            Box::new(|raw| {
                let storage = &mut raw["photography"]["assets"][DIGEST]["storage"];
                let engine = base64::engine::general_purpose::STANDARD;
                let mut bytes = engine.decode(storage["data"].as_str().unwrap()).unwrap();
                bytes[200] ^= 1;
                storage["data"] = json!(engine.encode(bytes));
            }),
        ),
        (
            "[missing-resource]",
            Box::new(|raw| raw["pages"][0]["layers"][0]["nodes"][0]["photo"] = json!("nobody")),
        ),
        (
            "[missing-resource]",
            Box::new(|raw| {
                raw["photography"]["photos"][0]["source"] =
                    json!(format!("sha256:{}", "1".repeat(64)))
            }),
        ),
        (
            "[missing-resource]",
            Box::new(|raw| {
                raw["photography"]["stacks"]["harbor-burst"]["photos"][1] = json!("nobody")
            }),
        ),
        (
            "[missing-resource]",
            Box::new(|raw| {
                raw["photography"]["photos"][0]["snapshots"][0]["variant"] = json!("nobody")
            }),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["photos"][0]["variants"][0]["id"] = json!("first")),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["photos"][1]["id"] = json!("hero")),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["photos"][0]["rating"] = json!(6)),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["photography"]["unknown"] = json!({})),
        ),
        (
            "[malformed-resource]",
            Box::new(|raw| raw["pages"][0]["layers"][0]["nodes"][0]["crop"] = json!([0, 0, 1, 1])),
        ),
        (
            "[unsupported-capability]",
            Box::new(|raw| {
                raw["photography"]["photos"][0]["variants"][0]["develop"]["process"] = json!(2)
            }),
        ),
        (
            "[unsafe-path]",
            Box::new(|raw| {
                raw["photography"]["assets"][DIGEST]["storage"] =
                    json!({"kind": "external", "path": "../capture.dng"})
            }),
        ),
    ];
    for (index, (code, change)) in cases.iter().enumerate() {
        let mut raw = base.clone();
        change(&mut raw);
        let error = pentool::transaction::validate_value(&raw)
            .expect_err(&format!("case {index}"))
            .to_string();
        assert!(error.starts_with(code), "case {index}: {error}");
    }
}
