use pentool::{composite, fill, image as assets, scene, transform};
use serde_json::{json, Value};
use std::path::Path;
fn doc() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 4)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"subject","x":1,"y":1,"width":2,"height":2,"style":{"fill":{"fallback":"#204060"}}}]);
    raw
}
fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new("target/transform.pen"), None, 1.0).unwrap()
}
#[test]
fn named_gradient_fills_are_reusable_and_presets_capture_only_dependencies() {
    let mut raw = doc();
    raw["styles"]["gradient"] = json!({"type":"fill","value":{"kind":"linear","x2":8,"stops":[{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]}});
    raw["styles"]["unused"] = json!({"type":"color","value":"#ff0000"});
    fill::edit(
        &mut raw,
        None,
        "add",
        "gradient-layer",
        &json!({"fill":{"ref":"gradient","fallback":{"kind":"solid","color":"#ff0000"}}}),
    )
    .unwrap();
    let before = render(&raw);
    let package =
        pentool::preset::capture(&raw, Path::new("target/fill.pen"), None, "gradient-layer")
            .unwrap();
    assert!(package["styles"].get("gradient").is_some());
    assert!(package["styles"].get("unused").is_none());
    raw["styles"]["gradient"]["value"]["stops"][1]["color"] = json!("#204060");
    assert_ne!(render(&raw), before);
    raw["styles"].as_object_mut().unwrap().remove("gradient");
    assert_eq!(render(&raw).get_pixel(7, 3).0, [255, 0, 0, 255]);
}
#[test]
fn transform_metadata_preserves_geometry_and_rolls_back_invalid_values() {
    let mut raw = doc();
    transform::edit(
        &mut raw,
        None,
        "subject",
        "add",
        "placement",
        Some("rotate"),
        &json!({"degrees":30}),
        None,
    )
    .unwrap();
    let matrix = raw["pages"][0]["layers"][0]["nodes"][0]["transforms"][0]["matrix"].clone();
    transform::metadata(
        &mut raw,
        None,
        "subject",
        "placement",
        &json!({"opacity":0.5}),
    )
    .unwrap();
    assert_eq!(
        raw["pages"][0]["layers"][0]["nodes"][0]["transforms"][0]["matrix"],
        matrix
    );
    let before = raw.clone();
    assert!(transform::metadata(
        &mut raw,
        None,
        "subject",
        "placement",
        &json!({"opacity":2})
    )
    .is_err());
    assert_eq!(raw, before);
    assert!(transform::metadata(
        &mut raw,
        None,
        "subject",
        "placement",
        &json!({"unknown":1})
    )
    .is_err());
    assert_eq!(raw, before);
}
#[test]
fn inverse_transform_stacks_sample_once_and_keep_source_data_unchanged() {
    let mut raw = doc();
    let original = render(&raw);
    transform::edit(
        &mut raw,
        None,
        "subject",
        "add",
        "up",
        Some("scale"),
        &json!({"x":2,"y":2}),
        None,
    )
    .unwrap();
    transform::edit(
        &mut raw,
        None,
        "subject",
        "add",
        "down",
        Some("scale"),
        &json!({"x":0.5,"y":0.5}),
        None,
    )
    .unwrap();
    assert_eq!(render(&raw), original);
    assert_eq!(raw["image_assets"], json!({}));
    let before = raw.clone();
    assert!(transform::edit(
        &mut raw,
        None,
        "subject",
        "add",
        "bad",
        Some("scale"),
        &json!({"x":0}),
        None
    )
    .is_err());
    assert_eq!(raw, before);
    transform::edit(
        &mut raw,
        None,
        "subject",
        "disable",
        "down",
        None,
        &json!({}),
        None,
    )
    .unwrap();
    assert_eq!(render(&raw).get_pixel(4, 3).0, [32, 64, 96, 255]);
}
#[test]
fn perspective_and_four_corner_have_exact_translation_and_reject_crossed_quads() {
    let mut raw = doc();
    transform::edit(
        &mut raw,
        None,
        "subject",
        "add",
        "screen",
        Some("perspective"),
        &json!({"quad":[[3,1],[5,1],[5,3],[3,3]]}),
        None,
    )
    .unwrap();
    assert_eq!(render(&raw).get_pixel(3, 1).0, [32, 64, 96, 255]);
    assert_eq!(render(&raw).get_pixel(1, 1).0, [0, 0, 0, 0]);
    let before = raw.clone();
    assert!(transform::edit(
        &mut raw,
        None,
        "subject",
        "set",
        "screen",
        Some("four-corner"),
        &json!({"quad":[[0,0],[4,4],[4,0],[0,4]]}),
        None
    )
    .is_err());
    assert_eq!(raw, before);
}
#[test]
fn fill_gradients_have_exact_pixels_and_explicit_token_and_color_spaces() {
    let mut raw = composite::migrate(scene::new_document(4, 1)).unwrap();
    raw["styles"] = json!({"ink":{"type":"color","value":"#ffffff"}});
    let gradient = json!({"kind":"linear","x1":0,"y1":0,"x2":4,"y2":0,"stops":[{"position":0,"color":"#000000"},{"position":1,"color":{"ref":"ink","fallback":"#ff0000"}}]});
    fill::edit(&mut raw, None, "add", "gradient", &json!({"fill":gradient})).unwrap();
    let image = render(&raw);
    for (x, value) in [32, 96, 159, 223].iter().enumerate() {
        assert_eq!(
            image.get_pixel(x as u32, 0).0,
            [*value, *value, *value, 255]
        );
    }
    raw["pages"][0]["layers"][0]["nodes"][0]["fill"]["space"] = json!("linear");
    assert!(render(&raw).get_pixel(0, 0)[0] > image.get_pixel(0, 0)[0]);
    for kind in ["radial", "conic"] {
        let fill = json!({"kind":kind,"cx":2,"cy":0,"stops":[{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]});
        fill::edit(&mut raw, None, "set", "gradient", &json!({"fill":fill})).unwrap();
        assert_eq!(render(&raw), render(&raw));
    }
}
#[test]
fn fill_coordinates_are_local_to_nonzero_bounds() {
    let mut raw = composite::migrate(scene::new_document(6, 1)).unwrap();
    fill::edit(&mut raw,None,"add","shifted",&json!({"x":2,"y":0,"width":4,"height":1,"fill":{"kind":"linear","x1":0,"y1":0,"x2":4,"y2":0,"stops":[{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]}})).unwrap();
    let image = render(&raw);
    assert_eq!(image.get_pixel(1, 0).0, [255, 255, 255, 255]);
    for (x, value) in [32, 96, 159, 223].iter().enumerate() {
        assert_eq!(
            image.get_pixel(x as u32 + 2, 0).0,
            [*value, *value, *value, 255]
        );
    }
}
#[test]
fn pattern_fills_repeat_verified_source_pixels_without_duplication() {
    let mut raw = composite::migrate(scene::new_document(4, 1)).unwrap();
    let mut pixels = image::RgbaImage::new(2, 1);
    pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    pixels.put_pixel(1, 0, image::Rgba([0, 0, 255, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let imported = assets::add(
        &mut raw,
        None,
        "layer-1",
        "source",
        &bytes,
        assets::embedded_storage(&bytes),
        0.0,
        0.0,
        2.0,
        1.0,
        assets::Fit::Fill,
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["visible"] = json!(false);
    fill::edit(
        &mut raw,
        None,
        "add",
        "pattern",
        &json!({"fill":{"kind":"pattern","asset":imported.asset}}),
    )
    .unwrap();
    let image = render(&raw);
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(1, 0).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(2, 0).0, [255, 0, 0, 255]);
    assert_eq!(raw["image_assets"].as_object().unwrap().len(), 1);
}
