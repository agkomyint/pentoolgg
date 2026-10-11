//! Native stored paints render through verified ephemeral RGB evaluation.
use pentool::{
    color::{
        catalog,
        scene::{slots, Scope},
    },
    composite, scene, transaction,
};
use serde_json::{json, Value};
use std::path::Path;

const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
fn doc() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
fn rgb(channels: [f64; 3]) -> Value {
    json!({"space":"srgb","channels":channels})
}
fn card(raw: &mut Value) -> &mut Value {
    &mut raw["pages"][0]["layers"][0]["nodes"][0]
}

#[test]
fn native_background_and_vector_paints_have_exact_pixels_and_preserve_original_values() {
    let mut raw = doc();
    raw["pages"][0]["canvas"]["background"] = json!({"space":"lab-d50","channels":[100,0,0]});
    card(&mut raw)["style"]["fill"]["fallback"] = rgb([0.2, 0.4, 0.6]);
    raw["custom"] = json!({"space":"extension-not-a-paint","channels":[7,8,9]});
    let before = raw.clone();
    scene::validate(&raw).unwrap();
    let paints = slots(&raw, Scope::Document).unwrap();
    assert_eq!(paints.len(), 2);
    let output = composite::render(&raw, Path::new("target/native-scene.pen"), None, 1.0).unwrap();
    assert_eq!(output.get_pixel(32, 24).0, [51, 102, 153, 255]);
    assert_eq!(output.get_pixel(0, 0).0, [255; 4]);
    assert_eq!(raw, before);
    assert!(catalog::downgrade(raw).is_err());
}

#[test]
fn icc_rgb_and_cmyk_paints_use_matching_header_and_requested_relative_paths() {
    let mut raw = doc();
    let path = Path::new("target/native-cmyk.pen");
    catalog::import(&mut raw, path, RGB, "rgb", "allowed", None, None).unwrap();
    card(&mut raw)["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"rgb","channels":[0.2,0.4,0.6]});
    let output = composite::render(&raw, path, None, 1.0).unwrap();
    let pixel = output.get_pixel(32, 24).0;
    assert!((i32::from(pixel[0]) - 124).abs() <= 1, "{pixel:?}");
    assert!((i32::from(pixel[1]) - 170).abs() <= 1, "{pixel:?}");
    assert!((i32::from(pixel[2]) - 203).abs() <= 1, "{pixel:?}");
    // Authored probe variant: expose its forward table as the relative path.
    let mut cmyk = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc").to_vec();
    let tags = u32::from_be_bytes(cmyk[128..132].try_into().unwrap()) as usize;
    for i in 0..tags {
        let start = 132 + i * 12;
        if &cmyk[start..start + 4] == b"A2B0" {
            cmyk[start..start + 4].copy_from_slice(b"A2B1");
        }
    }
    catalog::import(&mut raw, path, &cmyk, "press", "allowed", None, None).unwrap();
    card(&mut raw)["style"]["fill"]["fallback"] =
        json!({"space":"icc-cmyk","profile":"press","channels":[0,0,0,1]});
    let before = raw.clone();
    let output = composite::render(&raw, path, None, 1.0).unwrap();
    let pixel = output.get_pixel(32, 24).0;
    for channel in &pixel[..3] {
        assert!((i32::from(*channel) - 185).abs() <= 2, "{pixel:?}");
    }
    assert_eq!(raw, before);
    card(&mut raw)["style"]["fill"]["fallback"]["profile"] = json!("rgb");
    assert!(format!("{:#}", scene::validate(&raw).unwrap_err()).contains("object color-card"));
}

#[test]
fn native_named_styles_fill_stops_and_effect_colors_share_the_preview_path() {
    let mut raw = doc();
    raw["styles"]["brand"] = json!({"type":"color","value":rgb([1.0,0.0,0.0])});
    card(&mut raw)["style"]["fill"] = json!({"ref":"brand","fallback":rgb([0.0,0.0,1.0])});
    let path = Path::new("target/native-effects.pen");
    assert_eq!(
        composite::render(&raw, path, None, 1.0)
            .unwrap()
            .get_pixel(32, 24)
            .0,
        [255, 0, 0, 255]
    );
    card(&mut raw)["effects"] = json!([{"id":"overlay","kind":"color-overlay","enabled":true,"opacity":1,"blend_mode":"normal","params":{"color":rgb([0.0,1.0,0.0])}}]);
    assert_eq!(
        composite::render(&raw, path, None, 1.0)
            .unwrap()
            .get_pixel(32, 24)
            .0,
        [0, 255, 0, 255]
    );
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"gradient","x":0,"y":0,"width":64,"height":48,"fill":{"kind":"linear","space":"srgb","stops":[{"position":0,"color":rgb([1.0,0.0,0.0])},{"position":1,"color":rgb([0.0,0.0,1.0])}]}}]);
    let output = composite::render(&raw, path, None, 1.0).unwrap();
    let middle = output.get_pixel(32, 24).0;
    assert!((i32::from(middle[0]) - 128).abs() <= 3, "{middle:?}");
    assert!((i32::from(middle[2]) - 128).abs() <= 3, "{middle:?}");
    assert_eq!(middle[1], 0);
    let mut legacy = catalog::downgrade(doc()).unwrap();
    legacy["pages"][0]["layers"][0]["nodes"] = raw["pages"][0]["layers"][0]["nodes"].clone();
    assert!(scene::validate(&legacy).is_err());
}

#[test]
fn selected_page_does_not_load_unrelated_missing_external_profiles() {
    let root = std::env::temp_dir().join(format!(
        "pentool-native-pages-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("art.pen");
    std::fs::write(root.join("profile.icc"), RGB).unwrap();
    let mut raw = doc();
    catalog::import(
        &mut raw,
        &path,
        RGB,
        "external",
        "allowed",
        None,
        Some("profile.icc"),
    )
    .unwrap();
    let mut second = raw["pages"][0].clone();
    second["id"] = json!("second");
    second["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"external","channels":[0.2,0.4,0.6]});
    raw["pages"].as_array_mut().unwrap().push(second);
    std::fs::remove_file(root.join("profile.icc")).unwrap();
    scene::validate(&raw).unwrap();
    composite::render(&raw, &path, Some("page-1"), 1.0).unwrap();
    assert!(composite::render(&raw, &path, Some("second"), 1.0).is_err());
    let mut hidden = card(&mut raw).clone();
    hidden["id"] = json!("hidden");
    hidden["visible"] = json!(false);
    hidden["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"external","channels":[0.2,0.4,0.6]});
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(hidden);
    composite::render(&raw, &path, Some("page-1"), 1.0).unwrap();
    assert!(!root.join(".pentool").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_native_edits_fail_before_history_and_preserve_document_bytes() {
    let root = std::env::temp_dir().join(format!(
        "pentool-native-rollback-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("art.pen");
    let mut raw = doc();
    let before = serde_json::to_vec(&raw).unwrap();
    std::fs::write(&path, &before).unwrap();
    card(&mut raw)["style"]["fill"]["fallback"] =
        json!({"space":"icc-cmyk","profile":"missing","channels":[0,0,0,0]});
    let error = transaction::commit_value(&path, "native-test", false, None, &raw).unwrap_err();
    assert!(format!("{error:#}").contains("color-card"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!root.join(".pentool/history").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_paint_traversal_bounds_paths_counts_and_reference_nesting() {
    let mut raw = doc();
    let mut color = rgb([0.2, 0.4, 0.6]);
    for _ in 0..65 {
        color = json!({"fallback":color});
    }
    card(&mut raw)["style"]["fill"] = color;
    assert!(
        format!("{:#}", slots(&raw, Scope::Document).unwrap_err()).contains("nesting exceeds 64")
    );
    let mut raw = doc();
    raw["styles"]["s".repeat(4097)] = json!({"type":"color","value":rgb([0.1,0.2,0.3])});
    assert!(format!("{:#}", slots(&raw, Scope::Document).unwrap_err()).contains("path exceeds"));
    let mut raw = doc();
    let mut node = card(&mut raw).clone();
    node["style"]["fill"]["fallback"] = rgb([0.2, 0.4, 0.6]);
    raw["pages"][0]["layers"][0]["nodes"] =
        Value::Array(vec![node; pentool::color::scene::MAX_NATIVE_SLOTS + 1]);
    assert!(format!("{:#}", slots(&raw, Scope::Document).unwrap_err()).contains("65536 slots"));
}

#[test]
fn normative_native_fixture_is_hash_pinned_and_exports_without_changing_channels() {
    let bytes = include_bytes!("../docs/fixtures/print-v8-native.pen");
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    let entry = manifest["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["file"] == "print-v8-native.pen")
        .unwrap();
    assert_eq!(
        pentool::resource::sha256(bytes),
        format!("sha256:{}", entry["sha256"].as_str().unwrap())
    );
    let root = std::env::temp_dir().join(format!(
        "pentool-native-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("art.pen"), bytes).unwrap();
    for output in ["art.png", "art.svg", "art.pdf"] {
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&root)
            .args(["export", "art.pen", output])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let png = image::load_from_memory(&std::fs::read(root.join("art.png")).unwrap())
        .unwrap()
        .to_rgba8();
    assert_eq!(png.get_pixel(32, 24).0, [51, 102, 153, 255]);
    assert_eq!(png.get_pixel(0, 0).0, [255; 4]);
    assert!(std::fs::read(root.join("art.pdf"))
        .unwrap()
        .starts_with(b"%PDF-"));
    assert_eq!(std::fs::read(root.join("art.pen")).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}
