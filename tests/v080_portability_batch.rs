use pentool::{composite, linked, preset, scene};
use serde_json::json;
use std::path::Path;
#[test]
fn appearance_round_trip_keeps_ids_geometry_and_rejects_incompatible_packages() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"rect","id":"a","x":0,"y":0,"width":2,"height":2,"style":{"fill":{"fallback":"#123456"}}},
        {"kind":"rect","id":"b","x":4,"y":4,"width":3,"height":3,"style":{"fill":{"fallback":"#ffffff"}}}
    ]);
    let path = Path::new("target/preset.pen");
    let appearance = preset::capture(&raw, path, None, "a").unwrap();
    preset::apply(&mut raw, None, "b", &appearance).unwrap();
    assert_eq!(raw["pages"][0]["layers"][0]["nodes"][1]["x"], 4);
    assert_eq!(raw["pages"][0]["layers"][0]["nodes"][1]["id"], "b");
    assert_eq!(
        raw["pages"][0]["layers"][0]["nodes"][1]["style"],
        appearance["appearance"]["style"]
    );
    let before = raw.clone();
    let mut bad = appearance.clone();
    bad["engine"] = json!(99);
    assert!(preset::apply(&mut raw, None, "b", &bad).is_err());
    assert_eq!(raw, before);
    bad = appearance;
    bad["script"] = json!("anything");
    assert!(preset::apply(&mut raw, None, "b", &bad).is_err());
    assert_eq!(raw, before);
    raw["pages"][0]["layers"][0]["nodes"][1]["locked"] = json!(true);
    assert!(preset::apply(&mut raw, None, "b", &bad).is_err());
}
#[test]
fn composite_batch_rolls_back_all_prior_operations() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    let before = raw.clone();
    let operations = json!([
        {"type":"fill-add","id":"backdrop","settings":{"fill":{"kind":"solid","color":"#204060"}}},
        {"type":"adjustment-add","id":"grade","settings":{"adjustment":"invert"}},
        {"type":"effect-add","id":"missing","op_id":"shadow","settings":{"kind":"drop-shadow"}}
    ]);
    assert!(scene::apply_batch(&mut raw, None, operations.as_array().unwrap()).is_err());
    assert_eq!(raw, before);
    scene::apply_batch(&mut raw, None, &operations.as_array().unwrap()[..2]).unwrap();
    let pixels = composite::render(&raw, Path::new("target/batch.pen"), None, 1.0).unwrap();
    assert_eq!(pixels.get_pixel(4, 4).0, [223, 191, 159, 255]);
}
#[test]
fn collect_is_new_directory_only_and_identical_offline() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    let root = std::env::temp_dir().join(format!("pentool-v080-collect-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let output = root.join("portable");
    if output.exists() {
        std::fs::remove_dir_all(&output).unwrap();
    }
    let source = root.join("source.pen");
    let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([64, 128, 192, 255]));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let digest = pentool::resource::sha256(&bytes);
    std::fs::write(root.join("photo.png"), &bytes).unwrap();
    pentool::image::add(
        &mut raw,
        None,
        "layer-1",
        "photo",
        &bytes,
        pentool::image::external_storage(Path::new("photo.png")).unwrap(),
        0.0,
        0.0,
        8.0,
        8.0,
        pentool::image::Fit::Fill,
    )
    .unwrap();
    std::fs::write(&source, serde_json::to_vec(&raw).unwrap()).unwrap();
    linked::collect(&raw, &source, &output, true).unwrap();
    assert!(!output.exists());
    linked::collect(&raw, &source, &output, false).unwrap();
    assert_eq!(
        linked::report(&raw, &source).unwrap()["dependencies"][0]["status"],
        "verified"
    );
    assert_eq!(
        linked::locate(&raw, &source, &digest, &["photo.png".into()]).unwrap()["matches"],
        json!(["photo.png"])
    );
    std::fs::write(root.join("photo.png"), b"changed source").unwrap();
    assert_eq!(
        linked::report(&raw, &source).unwrap()["dependencies"][0]["status"],
        "stale"
    );
    let before = raw.clone();
    assert!(linked::edit(
        &mut raw,
        &source,
        "relink",
        &digest,
        Some(Path::new("photo.png")),
        true
    )
    .is_err());
    assert_eq!(raw, before);
    std::fs::write(root.join("photo.png"), &bytes).unwrap();
    assert!(linked::collect(&raw, &source, &output, false).is_err());
    let portable: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("document.pen")).unwrap()).unwrap();
    let expected = composite::render(&raw, &source, None, 1.0).unwrap();
    std::fs::remove_file(root.join("photo.png")).unwrap();
    assert_eq!(
        expected,
        composite::render(&portable, &output.join("document.pen"), None, 1.0).unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}
