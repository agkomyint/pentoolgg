use pentool::{composite, inspect, scene, selection};
use serde_json::{json, Value};
use std::path::Path;
fn doc() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"card","x":2,"y":2,"width":3,"height":3,"style":{"fill":{"fallback":"#204060"}}}]);
    raw
}
#[test]
fn queries_are_read_only_bounded_and_boolean() {
    let raw = doc();
    let original = raw.clone();
    let path = Path::new("target/selection.pen");
    let pixels = composite::render(&raw, path, None, 1.0).unwrap();
    let q = json!({"kind":"subtract","a":{"kind":"node-alpha","node":"card"},"b":{"kind":"rectangle","x":2,"y":2,"width":1,"height":3}});
    let coverage = selection::coverage(&raw, path, None, &q, &pixels).unwrap();
    assert_eq!(selection::bounds(&coverage, 8, 8), Some([3, 2, 2, 3]));
    assert_eq!(raw, original);
    assert!(selection::coverage(
        &raw,
        path,
        None,
        &json!({"kind":"rectangle","width":-1}),
        &pixels
    )
    .is_err());
    let mut deep = json!({"kind":"node-alpha","node":"card"});
    for _ in 0..18 {
        deep = json!({"kind":"add","a":deep,"b":{"kind":"node-alpha","node":"card"}});
    }
    assert!(selection::coverage(&raw, path, None, &deep, &pixels).is_err());
}
#[test]
fn saved_masks_crop_and_analysis_are_deterministic() {
    let mut raw = doc();
    let path = Path::new("target/selection.pen");
    let q = json!({"kind":"rectangle","x":2,"y":2,"width":3,"height":3});
    selection::save(&mut raw, path, None, "region", &q).unwrap();
    let before = raw.clone();
    let result = inspect::analyze(&raw, path, None, "page", Some(&q), &[[3, 3]]).unwrap();
    assert_eq!(result["selected_pixels"], 9);
    assert_eq!(result["samples"][0]["rgba"], json!([32, 64, 96, 255]));
    assert_eq!(result["histogram"]["red"][32], 2295);
    assert_eq!(raw, before);
    assert!(inspect::analyze(&raw, path, None, "page", None, &[[8, 0]]).is_err());
    assert_eq!(
        inspect::compare(&raw, path, None, "page", None).unwrap()["changed_pixels"],
        0
    );
    selection::crop(&mut raw, path, None, &q).unwrap();
    pentool::transaction::validate_value(&raw).unwrap();
    let cropped = composite::render(&raw, path, None, 1.0).unwrap();
    assert_eq!(cropped.dimensions(), (3, 3));
    assert!(cropped.pixels().all(|p| p.0 == [32, 64, 96, 255]));
}
#[test]
fn crop_preserves_page_space_gradient_overlay_pixels() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"surface","x":0,"y":0,"width":8,"height":8,"style":{"fill":{"fallback":"#ffffff"}},"effects":[{"id":"grade","kind":"gradient-overlay","enabled":true,"opacity":1,"blend_mode":"normal","params":{"fill":{"kind":"linear","x1":0,"y1":0,"x2":8,"y2":0,"stops":[{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]}}}]}]);
    let path = Path::new("target/crop-gradient.pen");
    let full = composite::render(&raw, path, None, 1.0).unwrap();
    selection::crop(
        &mut raw,
        path,
        None,
        &json!({"kind":"rectangle","x":2,"y":1,"width":4,"height":5}),
    )
    .unwrap();
    let cropped = composite::render(&raw, path, None, 1.0).unwrap();
    assert_eq!(cropped.dimensions(), (4, 5));
    for y in 0..5 {
        for x in 0..4 {
            assert_eq!(
                cropped.get_pixel(x, y),
                full.get_pixel(x + 2, y + 1),
                "{x},{y}"
            );
        }
    }
    assert_eq!(
        raw["pages"][0]["layers"][0]["nodes"][0]["children"][0]["effect_origin"],
        json!([2.0, 1.0])
    );
}
