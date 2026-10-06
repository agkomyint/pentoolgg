use pentool::{blend, composite, scene};
use serde_json::{json, Value};
use std::path::Path;

fn pixel(b: [u8; 4], s: [u8; 4], mode: &str, space: &str) -> [u8; 4] {
    let mut dst = image::RgbaImage::from_pixel(1, 1, image::Rgba(b));
    let src = image::RgbaImage::from_pixel(1, 1, image::Rgba(s));
    blend::over(&mut dst, &src, mode, space).unwrap();
    dst.get_pixel(0, 0).0
}
#[test]
fn exact_separable_and_nonseparable_blend_conformance() {
    // Analytic reference at backdrop=.2, source=.8, opaque encoded sRGB.
    let expected = [204, 41, 214, 82, 51, 204, 255, 0, 173, 89, 153, 173];
    for (mode, value) in blend::MODES[..12].iter().zip(expected) {
        assert_eq!(
            pixel([51, 51, 51, 255], [204, 204, 204, 255], mode, "srgb"),
            [value, value, value, 255],
            "{mode}"
        );
    }
    for (mode, expected) in [
        ("hue", [134, 83, 32, 255]),
        ("saturation", [51, 102, 153, 255]),
        ("color", [134, 83, 32, 255]),
        ("luminosity", [121, 172, 223, 255]),
    ] {
        assert_eq!(
            pixel([51, 102, 153, 255], [204, 153, 102, 255], mode, "srgb"),
            expected,
            "{mode}"
        );
    }
    for mode in blend::MODES {
        assert_eq!(
            pixel([7, 99, 211, 0], [31, 63, 95, 255], mode, "srgb"),
            [31, 63, 95, 255]
        );
        assert_eq!(
            pixel([7, 99, 211, 255], [31, 63, 95, 0], mode, "srgb"),
            [7, 99, 211, 255]
        );
        assert_eq!(
            pixel([255, 0, 0, 0], [0, 255, 0, 0], mode, "srgb"),
            [0, 0, 0, 0]
        );
    }
    assert_eq!(
        pixel([255, 0, 0, 128], [0, 0, 255, 128], "normal", "srgb"),
        [85, 0, 170, 192]
    );
    assert_eq!(
        pixel([255, 0, 0, 128], [0, 0, 255, 128], "multiply", "srgb"),
        [85, 0, 85, 192]
    );
    assert!(blend::validate("typo", "srgb").is_err());
}
#[test]
fn linear_conversion_round_trips_and_changes_blending_explicitly() {
    for byte in 0..=255u8 {
        let value = blend::to_srgb(blend::to_linear(f64::from(byte) / 255.0));
        assert_eq!((value * 255.0 + 0.5).floor() as u8, byte);
    }
    assert_ne!(
        pixel([255, 0, 0, 255], [0, 0, 255, 128], "normal", "srgb"),
        pixel([255, 0, 0, 255], [0, 0, 255, 128], "normal", "linear")
    );
    assert_eq!(
        pixel([255, 0, 0, 255], [0, 0, 255, 128], "normal", "linear"),
        [187, 0, 188, 255]
    );
}
fn rect(id: &str, width: u32, color: &str) -> Value {
    json!({"kind":"rect","id":id,"x":0,"y":0,"width":width,"height":4,"style":{"fill":{"fallback":color}}})
}
fn doc() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 4)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        rect("base", 4, "#ff0000"),
        rect("texture", 8, "#0000ff"),
        rect("finish", 8, "#00ff00")
    ]);
    raw
}
fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new("target/blend.pen"), None, 1.0).unwrap()
}

#[test]
fn consecutive_clipping_uses_base_alpha_and_rejects_broken_stacks_atomically() {
    let mut raw = doc();
    composite::clip_edit(&mut raw, None, "add", "texture", Some("base")).unwrap();
    composite::clip_edit(&mut raw, None, "add", "finish", Some("base")).unwrap();
    let result = render(&raw);
    assert_eq!(result.get_pixel(1, 1).0, [0, 255, 0, 255]);
    assert_eq!(result.get_pixel(6, 1).0, [0, 0, 0, 0]);
    let before = raw.clone();
    assert!(composite::clip_edit(&mut raw, None, "add", "finish", Some("texture")).is_err());
    assert_eq!(raw, before);
    raw["pages"][0]["layers"][0]["nodes"][0]["visible"] = json!(false);
    assert_eq!(render(&raw).get_pixel(1, 1).0, [0, 0, 0, 0]);
    let mut reordered = before;
    reordered["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(scene::validate(&reordered).is_err());
}

#[test]
fn group_isolation_and_pass_through_have_distinct_exact_pixels() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        rect("back",8,"#cc0000"),
        {"kind":"group","id":"group","children":[{
            "kind":"rect","id":"blue","x":0,"y":0,"width":8,"height":4,
            "blend_mode":"multiply","style":{"fill":{"fallback":"#0000cc"}}
        }]}
    ]);
    assert_eq!(render(&raw).get_pixel(1, 1).0, [0, 0, 204, 255]);
    raw["pages"][0]["layers"][0]["nodes"][1]["isolation"] = json!("pass-through");
    assert_eq!(render(&raw).get_pixel(1, 1).0, [0, 0, 0, 255]);
    raw["pages"][0]["layers"][0]["nodes"][1]["opacity"] = json!(0.5);
    assert!(scene::validate(&raw).is_err());
}
