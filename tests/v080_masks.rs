use pentool::{composite, mask, scene};
use serde_json::{json, Value};
use std::path::Path;
fn rect(id: &str, width: u32, color: &str) -> Value {
    json!({"kind":"rect","id":id,"x":0,"y":0,"width":width,"height":4,"style":{"fill":{"fallback":color}}})
}
fn doc() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 4)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] =
        json!([rect("shape", 4, "#000000"), rect("photo", 8, "#204060")]);
    raw
}
fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new("target/mask.pen"), None, 1.0).unwrap()
}

#[test]
fn masks_are_shared_hash_verified_and_reversible() {
    let mut raw = doc();
    let first = mask::create(
        &mut raw,
        Path::new("target/mask.pen"),
        None,
        "first",
        "vector:shape",
    )
    .unwrap();
    let second = mask::create(
        &mut raw,
        Path::new("target/mask.pen"),
        None,
        "second",
        "vector:shape",
    )
    .unwrap();
    assert_eq!(first["resource"], second["resource"]);
    assert_eq!(raw["mask_resources"].as_object().unwrap().len(), 1);
    mask::attach(&mut raw, None, "photo", "first", &json!({})).unwrap();
    let image = render(&raw);
    assert_eq!(image.get_pixel(1, 1).0, [32, 64, 96, 255]);
    assert_eq!(image.get_pixel(6, 1).0, [0, 0, 0, 0]);
    mask::delete(&mut raw, "first").unwrap();
    assert_eq!(render(&raw), image);
    mask::detach(&mut raw, None, "photo").unwrap();
    assert_eq!(render(&raw).get_pixel(6, 1).0, [32, 64, 96, 255]);
    let mut corrupt = raw.clone();
    corrupt["mask_resources"][first["resource"].as_str().unwrap()]["node"]["width"] = json!(2);
    assert!(scene::validate(&corrupt).is_err());
}

#[test]
fn raster_masks_density_invert_and_feather_are_deterministic() {
    let mut raw = doc();
    mask::create(
        &mut raw,
        Path::new("target/mask.pen"),
        None,
        "raster",
        "node-alpha:shape",
    )
    .unwrap();
    mask::attach(
        &mut raw,
        None,
        "photo",
        "raster",
        &json!({"invert":true,"density":0.5}),
    )
    .unwrap();
    let image = render(&raw);
    // The original black shape remains below the half-masked photograph.
    assert_eq!(image.get_pixel(1, 1).0, [16, 32, 48, 255]);
    assert_eq!(image.get_pixel(6, 1).0, [32, 64, 96, 255]);
    mask::attach(&mut raw, None, "photo", "raster", &json!({"feather":1})).unwrap();
    let image = render(&raw);
    assert!(image.get_pixel(4, 1)[3] > 0 && image.get_pixel(4, 1)[3] < 255);
    assert_eq!(image, render(&raw));
}

#[test]
fn linked_and_unlinked_masks_follow_explicit_transform_semantics() {
    let mut raw = doc();
    mask::create(
        &mut raw,
        Path::new("target/mask.pen"),
        None,
        "m",
        "vector:shape",
    )
    .unwrap();
    mask::attach(&mut raw, None, "photo", "m", &json!({})).unwrap();
    raw["pages"][0]["layers"][0]["nodes"][1]["transform"] = json!([1, 0, 0, 1, 2, 0]);
    let linked = render(&raw);
    assert_eq!(linked.get_pixel(5, 1).0, [32, 64, 96, 255]);
    raw["pages"][0]["layers"][0]["nodes"][1]["mask"]["linked"] = json!(false);
    let unlinked = render(&raw);
    assert_eq!(unlinked.get_pixel(5, 1).0, [0, 0, 0, 0]);
}

#[test]
fn masks_grade_adjustments_and_explicit_apply_preserves_pixels_and_id() {
    let mut raw = doc();
    mask::create(
        &mut raw,
        Path::new("target/mask.pen"),
        None,
        "m",
        "vector:shape",
    )
    .unwrap();
    composite::edit(
        &mut raw,
        None,
        "add",
        "grade",
        &json!({"adjustment":"invert"}),
    )
    .unwrap();
    mask::attach(&mut raw, None, "grade", "m", &json!({})).unwrap();
    assert_eq!(render(&raw).get_pixel(1, 1).0, [223, 191, 159, 255]);
    assert_eq!(render(&raw).get_pixel(6, 1).0, [32, 64, 96, 255]);
    composite::edit(&mut raw, None, "remove", "grade", &json!({})).unwrap();
    mask::attach(&mut raw, None, "photo", "m", &json!({})).unwrap();
    let before = render(&raw);
    mask::apply(&mut raw, Path::new("target/mask.pen"), None, "photo").unwrap();
    assert_eq!(render(&raw), before);
    assert_eq!(raw["pages"][0]["layers"][0]["nodes"][1]["id"], "photo");
    assert!(raw["pages"][0]["layers"][0]["nodes"][1]
        .get("mask")
        .is_none());
    let mut v5 = raw.clone();
    v5["version"] = json!(5);
    v5["pages"][0]["layers"][0]["nodes"][1]["mask"] = json!({"resource":"invalid"});
    assert!(scene::validate(&v5).is_err());
}
