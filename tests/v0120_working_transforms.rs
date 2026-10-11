//! Non-affine transform stacks resample float surfaces at the released byte
//! positions with the same premultiplied encoded-sRGB kernel, without rounding.
use pentool::composite;
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/working-transform.pen";
fn doc(nodes: Vec<Value>, size: u32) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":size,"height":size,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw
}
fn paint(channels: [f64; 3], alpha: f64, rect: [u32; 4]) -> Value {
    json!({"kind":"fill","id":"paint","x":rect[0],"y":rect[1],"width":rect[2],"height":rect[3],
        "fill":{"kind":"solid","color":{"space":"srgb","channels":channels,"alpha":alpha}}})
}
fn op(id: &str, matrix: [f64; 9], opacity: f64) -> Value {
    json!({"id":id,"kind":"matrix","enabled":true,"opacity":opacity,"matrix":matrix})
}
fn encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
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
fn non_affine_stacks_follow_the_released_byte_resampler_without_quantization() {
    let stacks = [
        vec![op(
            "tilt",
            [1.0, 0.1, 0.0, 0.0, 1.0, 0.0, 0.02, 0.01, 1.0],
            1.0,
        )],
        vec![
            op("shear", [1.0, 0.3, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0], 0.6),
            op(
                "tilt",
                [0.9, 0.0, 0.5, 0.1, 1.1, 0.0, 0.01, -0.02, 1.0],
                0.35,
            ),
        ],
        vec![
            op("off", [2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0], 1.0),
            op("tilt", [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.03, 0.0, 1.0], 1.0),
        ],
    ];
    for (index, stack) in stacks.into_iter().enumerate() {
        for scale in [1.0_f32, 2.0] {
            let mut node = paint([0.9, 0.3, 0.1], 0.7, [2, 2, 6, 5]);
            node["transforms"] = json!(stack);
            if index == 2 {
                node["transforms"][0]["enabled"] = json!(false);
            }
            let raw = doc(vec![node], 10);
            let before = raw.clone();
            let float = composite::render_working(&raw, Path::new(DOCUMENT), None, scale, false)
                .unwrap_or_else(|e| panic!("stack {index}: {e:#}"));
            let byte = composite::render(&raw, Path::new(DOCUMENT), None, scale).unwrap();
            assert_eq!(raw, before);
            let mut partial = 0;
            for (f, b) in float.surface.pixels().iter().zip(byte.pixels()) {
                let ba = f64::from(b[3]) / 255.0;
                assert!((f[3] - ba).abs() <= 1.0 / 255.0, "alpha {f:?} {b:?}");
                for c in 0..3 {
                    let premultiplied = encode(f[c]) * f[3];
                    let expected = f64::from(b[c]) / 255.0 * ba;
                    assert!(
                        (premultiplied - expected).abs() <= 2.0 / 255.0,
                        "color {f:?} {b:?}"
                    );
                }
                partial += usize::from(f[3] > 0.0 && f[3] < 0.69);
            }
            assert!(partial > 0, "stack {index} resampled no edge");
        }
    }
}

#[test]
fn weighted_half_pixel_shift_keeps_fractional_alpha_and_color() {
    // Opacity 0.5 forces per-operation resampling: a one-pixel translation
    // moves the sample by half a pixel, splitting coverage over two pixels.
    let alpha = 0.37123456789;
    let mut node = paint([0.2, 0.4, 0.8], alpha, [1, 0, 1, 1]);
    node["transforms"] = json!([op(
        "half",
        [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        0.5
    )]);
    let mut raw = doc(vec![node], 3);
    raw["pages"][0]["canvas"]["height"] = json!(1);
    let result = composite::render_working(&raw, Path::new(DOCUMENT), None, 1.0, false).unwrap();
    let pixels = result.surface.pixels();
    assert_eq!(pixels[0], [0.0; 4]);
    for pixel in &pixels[1..] {
        near(pixel, &[decode(0.2), decode(0.4), decode(0.8), alpha * 0.5]);
    }
}

#[test]
fn disabled_and_singular_stacks_keep_existing_semantics() {
    let mut node = paint([0.2, 0.4, 0.8], 1.0, [0, 0, 2, 2]);
    node["transforms"] = json!([op(
        "tilt",
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 0.0, 1.0],
        1.0
    )]);
    node["transforms"][0]["enabled"] = json!(false);
    let raw = doc(vec![node.clone()], 2);
    let pixels = composite::render_working(&raw, Path::new(DOCUMENT), None, 1.0, false)
        .unwrap()
        .surface
        .pixels()
        .to_vec();
    for pixel in pixels {
        near(&pixel, &[decode(0.2), decode(0.4), decode(0.8), 1.0]);
    }
    // A singular matrix fails in both precisions without mutating the source.
    node["transforms"][0]["enabled"] = json!(true);
    node["transforms"][0]["matrix"] = json!([1, 0, 0, 0, 0, 0, 0.5, 0, 1]);
    let raw = doc(vec![node], 2);
    let before = raw.clone();
    let float = composite::render_working(&raw, Path::new(DOCUMENT), None, 1.0, false);
    let byte = composite::render(&raw, Path::new(DOCUMENT), None, 1.0);
    assert_eq!(float.is_err(), byte.is_err());
    assert_eq!(raw, before);
}
