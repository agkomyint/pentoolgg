use pentool::{composite, effects, scene};
use serde_json::{json, Value};
use std::path::Path;
fn doc() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"card","x":2,"y":2,"width":3,"height":3,"style":{"fill":{"fallback":"#204060"}}}]);
    raw
}
fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new("target/effect.pen"), None, 1.0).unwrap()
}
#[test]
fn drop_shadow_has_exact_offset_pixels_and_content_opacity_is_independent() {
    let mut raw = doc();
    effects::edit(
        &mut raw,
        None,
        "card",
        "add",
        "shadow",
        &json!({"kind":"drop-shadow","params":{"x":2,"y":0,"blur":0,"color":"#ff0000"}}),
    )
    .unwrap();
    let image = render(&raw);
    assert_eq!(image.get_pixel(3, 3).0, [32, 64, 96, 255]);
    assert_eq!(image.get_pixel(5, 3).0, [255, 0, 0, 255]);
    raw["pages"][0]["layers"][0]["nodes"][0]["content_opacity"] = json!(0);
    assert_eq!(render(&raw).get_pixel(5, 3).0, [255, 0, 0, 255]);
    assert_eq!(render(&raw).get_pixel(2, 3).0, [0, 0, 0, 0]);
    raw["pages"][0]["layers"][0]["nodes"][0]["opacity"] = json!(0.5);
    assert_eq!(render(&raw).get_pixel(5, 3).0, [255, 0, 0, 128]);
}
#[test]
fn all_effect_kinds_validate_toggle_and_preserve_sources() {
    for kind in effects::KINDS {
        let mut raw = doc();
        let assets = raw["image_assets"].clone();
        let params = if kind == "gradient-overlay" {
            json!({"fill":{"kind":"linear","stops":[{"position":0,"color":"#000000"},{"position":1,"color":"#ffffff"}]}})
        } else {
            json!({})
        };
        effects::edit(
            &mut raw,
            None,
            "card",
            "add",
            "effect",
            &json!({"kind":kind,"params":params}),
        )
        .unwrap();
        assert_eq!(render(&raw), render(&raw));
        assert_eq!(raw["image_assets"], assets);
        effects::edit(&mut raw, None, "card", "disable", "effect", &json!({})).unwrap();
        assert_eq!(render(&raw), render(&doc()));
    }
}
#[test]
fn shared_effect_style_changes_all_references_and_bad_edits_roll_back() {
    let mut raw = doc();
    effects::edit(
        &mut raw,
        None,
        "card",
        "add",
        "overlay",
        &json!({"kind":"color-overlay","params":{"color":"#ff0000"}}),
    )
    .unwrap();
    let stack = raw["pages"][0]["layers"][0]["nodes"][0]["effects"].clone();
    raw["styles"]["appearance"] = json!({"type":"effects","value":stack});
    raw["pages"][0]["layers"][0]["nodes"][0]["effects"] = json!({"ref":"appearance","fallback":[]});
    assert_eq!(render(&raw).get_pixel(3, 3).0, [255, 0, 0, 255]);
    raw["styles"]["appearance"]["value"][0]["params"]["color"] = json!("#00ff00");
    assert_eq!(render(&raw).get_pixel(3, 3).0, [0, 255, 0, 255]);
    let before = raw.clone();
    assert!(effects::edit(
        &mut raw,
        None,
        "card",
        "add",
        "bad",
        &json!({"kind":"blur"})
    )
    .is_err());
    assert_eq!(raw, before);
}
