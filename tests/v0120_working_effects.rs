//! Overlay effects use native float paints and released silhouette/order rules.
use pentool::composite;
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/working-effect.pen";
fn doc(nodes: Vec<Value>) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":1,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw
}
fn paint() -> Value {
    json!({"kind":"fill","id":"base","x":0,"y":0,"width":1,"height":1,
        "fill":{"kind":"solid","color":{"space":"srgb","channels":[0.2,0.4,0.8],"alpha":0.37123456789}}})
}
fn overlay(id: &str, rgb: [f64; 3], alpha: f64, opacity: f64) -> Value {
    json!({"kind":"color-overlay","id":id,"enabled":true,"opacity":opacity,"blend_mode":"normal",
        "params":{"color":{"space":"srgb","channels":rgb,"alpha":alpha}}})
}
fn render(raw: &Value) -> composite::WorkingRender {
    composite::render_working(raw, Path::new(DOCUMENT), None, 1.0, false).unwrap()
}
fn decode(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn near(actual: &[f64], expected: &[f64]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 2e-12, "{actual:?} != {expected:?}");
    }
}
#[test]
fn native_color_overlay_preserves_fractional_color_content_and_final_opacity() {
    for content in [0.0_f64, 0.23, 1.0] {
        let mut node = paint();
        node["content_opacity"] = json!(content);
        node["opacity"] = json!(0.9);
        node["effects"] = json!([overlay("overlay", [0.8, 0.6, 0.2], 0.45123456789, 0.67)]);
        let raw = doc(vec![node]);
        let before = raw.clone();
        let amount = 0.45123456789 * 0.67;
        let expected = std::array::from_fn::<_, 3, _>(|i| {
            let b = [0.2, 0.4, 0.8][i];
            let s = [0.8, 0.6, 0.2][i];
            decode(if content == 0.0 {
                s
            } else {
                b * (1.0 - amount) + s * amount
            })
        });
        near(
            &render(&raw).surface.pixels()[0],
            &[
                expected[0],
                expected[1],
                expected[2],
                0.37123456789 * content.max(amount) * 0.9,
            ],
        );
        assert_eq!(raw, before);
    }
}
#[test]
fn ordered_overlays_use_immutable_source_alpha() {
    let mut node = paint();
    node["content_opacity"] = json!(0.1);
    node["effects"] = json!([
        overlay("one", [1.0, 0.0, 0.0], 0.2, 1.0),
        overlay("two", [0.0, 1.0, 0.0], 0.4, 1.0),
    ]);
    let expected = [0.2, 0.4, 0.8].map(|v| v * 0.8);
    let expected = [expected[0] + 0.2, expected[1], expected[2]];
    let expected = [
        decode(expected[0] * 0.6),
        decode(expected[1] * 0.6 + 0.4),
        decode(expected[2] * 0.6),
    ];
    near(
        &render(&doc(vec![node])).surface.pixels()[0],
        &[expected[0], expected[1], expected[2], 0.37123456789 * 0.4],
    );
}
#[test]
fn native_gradient_overlay_samples_float_fill_geometry_and_separate_alpha() {
    let mut node = paint();
    node["width"] = json!(2);
    node["effects"] = json!([{"kind":"gradient-overlay","id":"gradient","enabled":true,"opacity":1,"blend_mode":"normal",
        "params":{"fill":{"kind":"linear","x1":0,"y1":0,"x2":2,"y2":0,"space":{"space":"srgb:linear"},"intent":"relative-colorimetric",
        "stops":[{"position":0,"color":{"space":"srgb:linear","channels":[0,0,0],"alpha":1}},
            {"position":1,"color":{"space":"srgb:linear","channels":[1,1,1],"alpha":1}}]}}}]);
    let mut raw = doc(vec![node]);
    raw["pages"][0]["canvas"]["width"] = json!(2);
    let result = render(&raw);
    near(
        &result.surface.pixels()[0],
        &[0.25, 0.25, 0.25, 0.37123456789],
    );
    near(
        &result.surface.pixels()[1],
        &[0.75, 0.75, 0.75, 0.37123456789],
    );
    assert!(!result.conversion_clipped);
}
fn encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn spatial(kind: &str, params: Value, mode: &str, scale: f32) -> (Value, f32) {
    let mut node = paint();
    node["fill"]["color"]["alpha"] = json!(0.8);
    for (key, value) in [("x", 3), ("y", 3), ("width", 3), ("height", 3)] {
        node[key] = json!(value);
    }
    node["effects"] = json!([{"kind":kind,"id":"spatial","enabled":true,"opacity":0.8,
        "blend_mode":mode,"params":params}]);
    let mut raw = doc(vec![node]);
    raw["pages"][0]["canvas"]["width"] = json!(9);
    raw["pages"][0]["canvas"]["height"] = json!(9);
    (raw, scale)
}
fn cases() -> Vec<(&'static str, Value)> {
    vec![
        ("blur", json!({"radius":1.5})),
        (
            "drop-shadow",
            json!({"x":1,"y":2,"blur":1,"color":"#ff000080"}),
        ),
        (
            "inner-shadow",
            json!({"x":1,"y":1,"blur":1,"color":"#00ff00"}),
        ),
        ("outer-glow", json!({"blur":2,"color":"#ffff00"})),
        ("inner-glow", json!({"blur":1,"color":"#ffffff"})),
        ("stroke", json!({"size":1,"color":"#204080"})),
        (
            "stroke",
            json!({"size":1,"color":"#204080","position":"inner"}),
        ),
        (
            "stroke",
            json!({"size":2,"color":"#204080","position":"center"}),
        ),
    ]
}
#[test]
fn spatial_effects_follow_the_released_byte_engine_without_quantization() {
    for (kind, params) in cases() {
        for mode in ["normal", "multiply"] {
            for scale in [1.0_f32, 2.0] {
                let (raw, scale) = spatial(kind, params.clone(), mode, scale);
                let before = raw.clone();
                let float =
                    composite::render_working(&raw, Path::new(DOCUMENT), None, scale, false)
                        .unwrap_or_else(|e| panic!("{kind} {mode}: {e:#}"));
                let byte = composite::render(&raw, Path::new(DOCUMENT), None, scale).unwrap();
                assert_eq!(raw, before);
                let mut visible = 0;
                for (f, b) in float.surface.pixels().iter().zip(byte.pixels()) {
                    let ba = f64::from(b[3]) / 255.0;
                    assert!(
                        (f[3] - ba).abs() <= 2.0 / 255.0,
                        "{kind} {mode} alpha {f:?} {b:?}"
                    );
                    for c in 0..3 {
                        let premultiplied = encode(f[c]) * f[3];
                        let expected = f64::from(b[c]) / 255.0 * ba;
                        assert!(
                            (premultiplied - expected).abs() <= 3.0 / 255.0,
                            "{kind} {mode} color {f:?} {b:?}"
                        );
                    }
                    visible += usize::from(f[3] > 0.0);
                }
                assert!(visible > 0, "{kind} rendered nothing");
            }
        }
    }
}
#[test]
fn spatial_effects_keep_fractional_coverage_and_exact_geometry() {
    let line = |effects: Value| {
        let mut node = paint();
        node["x"] = json!(1);
        node["effects"] = effects;
        let mut raw = doc(vec![node]);
        raw["pages"][0]["canvas"]["width"] = json!(3);
        render(&raw).surface.pixels().to_vec()
    };
    let content = [decode(0.2), decode(0.4), decode(0.8), 0.37123456789];
    let color = |alpha: f64| json!({"space":"srgb","channels":[0.9,0.1,0.3],"alpha":alpha});
    let paint = [decode(0.9), decode(0.1), decode(0.3)];

    // A zero-radius blur is the identity, apart from sRGB round-trip roundoff.
    let blurred = line(json!([{"kind":"blur","id":"b","enabled":true,"opacity":1,
        "blend_mode":"normal","params":{"radius":0}}]));
    near(&blurred[1], &content);
    assert_eq!(blurred[0], [0.0; 4]);

    // An unblurred shadow one pixel right keeps the fractional source silhouette,
    // paint alpha, and effect opacity without byte quantization.
    let shadow = line(
        json!([{"kind":"drop-shadow","id":"s","enabled":true,"opacity":0.6,
        "blend_mode":"normal","params":{"x":1,"y":0,"blur":0,"color":color(0.41234567)}}]),
    );
    assert_eq!(shadow[0], [0.0; 4]);
    near(&shadow[1], &content);
    near(
        &shadow[2],
        &[
            paint[0],
            paint[1],
            paint[2],
            0.37123456789 * 0.41234567 * 0.6,
        ],
    );

    // An outer stroke covers both neighbours and leaves the content untouched.
    let stroke = line(
        json!([{"kind":"stroke","id":"k","enabled":true,"opacity":0.5,
        "blend_mode":"normal","params":{"size":1,"color":color(0.81234567)}}]),
    );
    for index in [0, 2] {
        near(
            &stroke[index],
            &[paint[0], paint[1], paint[2], 0.81234567 * 0.5],
        );
    }
    near(&stroke[1], &content);
}
#[test]
fn disabled_spatial_effects_skip_and_oversized_blur_fails_without_mutation() {
    for (kind, params) in cases() {
        let mut node = paint();
        node["effects"] = json!([{"kind":kind,"id":"spatial","enabled":false,"opacity":1,
            "blend_mode":"normal","params":params}]);
        near(
            &render(&doc(vec![node])).surface.pixels()[0],
            &[decode(0.2), decode(0.4), decode(0.8), 0.37123456789],
        );
    }
    let mut node = paint();
    node["effects"] = json!([{"kind":"blur","id":"b","enabled":true,"opacity":1,
        "blend_mode":"normal","params":{"radius":200}}]);
    let raw = doc(vec![node]);
    let before = raw.clone();
    let error = composite::render_working(&raw, Path::new(DOCUMENT), None, 2.0, false)
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("exceeds 256 pixels"));
    assert_eq!(raw, before);
}
