//! Float adjustment regression coverage uses independent encoded-channel math.
use pentool::{
    color::value::{Destination, Evaluator, NativeColor, ProfileSet},
    composite,
};
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/working-adjustment.pen";
fn doc(nodes: Vec<Value>) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":1,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw
}
fn paint(id: &str, rgb: [f64; 3], alpha: f64) -> Value {
    json!({"kind":"fill","id":id,"x":0,"y":0,"width":1,"height":1,"blend_space":"srgb",
        "fill":{"kind":"solid","color":{"space":"srgb","channels":rgb,"alpha":alpha}}})
}
fn adjustment(kind: &str, params: Value) -> Value {
    json!({"kind":"adjustment","id":"grade","adjustment":kind,"params":params,"enabled":true,
        "opacity":1,"blend_mode":"normal","blend_space":"srgb","scope":{"kind":"below"}})
}
fn render(raw: &Value) -> composite::WorkingRender {
    composite::render_working(raw, Path::new(DOCUMENT), None, 1.0, false).unwrap()
}
fn decode(v: f64) -> f64 {
    let a = v.abs();
    v.signum()
        * if a <= 0.04045 {
            a / 12.92
        } else {
            ((a + 0.055) / 1.055).powf(2.4)
        }
}
fn near(actual: &[f64], expected: &[f64]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 2e-12, "{actual:?} != {expected:?}");
    }
}
#[test]
fn all_thirteen_adjustments_keep_float_color_and_alpha() {
    let rgb = [0.2, 0.4, 0.8];
    let y = 0.2 * 0.2126 + 0.4 * 0.7152 + 0.8 * 0.0722;
    let factor = 259.0 * (25.5 + 255.0) / (255.0 * (259.0 - 25.5));
    let cases = [
        (
            "brightness-contrast",
            json!({"brightness":5,"contrast":10}),
            rgb.map(|v| factor * (v - 128.0 / 255.0) + 128.0 / 255.0 + 0.05),
        ),
        (
            "levels",
            json!({"black":25.5,"white":229.5,"gamma":1}),
            rgb.map(|v| (v - 0.1) / 0.8),
        ),
        (
            "curves",
            json!({"points":[[0,255],[255,0]]}),
            [0.8, 0.6, 0.2],
        ),
        ("hue-saturation", json!({"hue":120}), [0.8, 0.2, 0.4]),
        ("exposure", json!({"stops":1}), [0.4, 0.8, 1.6]),
        ("invert", json!({}), [0.8, 0.6, 0.2]),
        (
            "posterize",
            json!({"levels":4}),
            [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0],
        ),
        ("threshold", json!({"level":128}), [0.0; 3]),
        (
            "color-balance",
            json!({"red":10,"green":-10,"blue":5}),
            [0.3, 0.3, 0.85],
        ),
        ("black-and-white", json!({}), [y; 3]),
        (
            "channel-mixer",
            json!({"matrix":[[0,0,1,0],[1,0,0,0],[0,1,0,0.1]]}),
            [0.8, 0.2, 0.5],
        ),
        (
            "vibrance",
            json!({"amount":50}),
            rgb.map(|v| y + (v - y) * 1.2),
        ),
        (
            "gradient-map",
            json!({"stops":[{"position":0,"rgb":[0,255,0]},{"position":1,"rgb":[255,0,255]}]}),
            [y, 1.0 - y, y],
        ),
    ];
    for (kind, params, expected) in cases {
        let raw = doc(vec![
            paint("base", rgb, 0.37123456789),
            adjustment(kind, params),
        ]);
        let before = raw.clone();
        let result = render(&raw);
        near(
            &result.surface.pixels()[0],
            &[
                decode(expected[0]),
                decode(expected[1]),
                decode(expected[2]),
                0.37123456789,
            ],
        );
        assert_eq!(raw, before, "{kind}");
    }
}
#[test]
fn declared_blend_space_and_opacity_preserve_released_coordinate_semantics() {
    for space in ["srgb", "linear"] {
        let mut grade = adjustment("invert", json!({}));
        grade["blend_mode"] = json!("multiply");
        grade["blend_space"] = json!(space);
        grade["opacity"] = json!(0.123456789);
        let raw = doc(vec![paint("base", [0.2, 0.4, 0.8], 0.7), grade]);
        let expected = [0.2, 0.4, 0.8].map(|v| {
            let mixed = if space == "linear" {
                let l = decode(v) * decode(1.0 - v);
                if l <= 0.0031308 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                }
            } else {
                v * (1.0 - v)
            };
            decode(v * (1.0 - 0.123456789) + mixed * 0.123456789)
        });
        near(
            &render(&raw).surface.pixels()[0],
            &[expected[0], expected[1], expected[2], 0.7],
        );
    }
}
#[test]
fn alternate_working_primaries_adjust_encoded_srgb_without_reinterpreting_channels() {
    let mut raw = doc(vec![
        paint("base", [0.2, 0.4, 0.8], 0.7),
        adjustment("invert", json!({})),
    ]);
    raw["color_management"]["roles"] = json!({"working":{"destination":{"space":"display-p3:linear"},"intent":"relative-colorimetric"}});
    let profiles = ProfileSet::default();
    let mut engine =
        Evaluator::new(&profiles, pentool::color::icc::Intent::RelativeColorimetric).unwrap();
    let color =
        NativeColor::parse(&json!({"space":"srgb","channels":[0.8,0.6,0.2],"alpha":0.7})).unwrap();
    let destination = Destination::parse(&json!({"space":"display-p3:linear"})).unwrap();
    let expected = engine.working(&color, &destination).unwrap();
    near(&render(&raw).surface.pixels()[0], &expected.rgba);
}
#[test]
fn target_and_group_scopes_follow_visible_contribution_without_alpha_quantization() {
    for group in [false, true] {
        let mut target = paint("target", [0.4; 3], 0.25);
        if group {
            target = json!({"kind":"group","id":"group","children":[target]});
        }
        let mut grade = adjustment("invert", json!({}));
        grade["scope"] = if group {
            json!({"kind":"group","id":"group"})
        } else {
            json!({"kind":"targets","ids":["target"]})
        };
        let raw = doc(vec![target, paint("occluder", [0.2; 3], 0.5), grade]);
        // Visible target contribution is 0.25*(1-0.5)=0.125.
        let before = (0.2 * 0.5 + 0.4 * 0.25 * 0.5) / 0.625;
        let expected = decode(before * 0.875 + (1.0 - before) * 0.125);
        near(
            &render(&raw).surface.pixels()[0],
            &[expected, expected, expected, 0.625],
        );
    }
}

#[test]
fn masks_modulate_adjustment_strength_while_preserving_working_alpha_and_precision() {
    let mut raw = doc(vec![
        paint("base", [0.123456789, 0.4, 0.8], 0.734567891),
        adjustment("invert", json!({})),
        json!({"kind":"rect","id":"mask-source","x":0,"y":0,"width":1,"height":1,
            "style":{"fill":{"fallback":"#ffffff"}}}),
    ]);
    raw["pages"][0]["canvas"]["width"] = json!(2);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(2);
    pentool::mask::create(
        &mut raw,
        Path::new(DOCUMENT),
        None,
        "half",
        "vector:mask-source",
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .pop();
    pentool::mask::attach(
        &mut raw,
        None,
        "grade",
        "half",
        &json!({"linked":false,"density":0.5}),
    )
    .unwrap();
    let result = render(&raw);
    near(
        &result.surface.pixels()[0],
        &[
            decode(1.0 - 0.123456789),
            decode(0.6),
            decode(0.2),
            0.734567891,
        ],
    );
    let expected = [0.123456789, 0.4, 0.8]
        .map(|v| decode(v * (1.0 - 128.0 / 255.0) + (1.0 - v) * 128.0 / 255.0));
    near(
        &result.surface.pixels()[1],
        &[expected[0], expected[1], expected[2], 0.734567891],
    );
}

#[test]
fn disabled_adjustments_and_zero_strength_skip_extended_hsv_domain_checks() {
    let mut grade = adjustment("hue-saturation", json!({"hue":120}));
    grade["enabled"] = json!(false);
    let mut base = paint("base", [0.2, 0.4, 0.8], 0.7);
    base["fill"]["color"] = json!({"space":"lab-d50","channels":[80,100,-100],"alpha":0.7});
    let expected = render(&doc(vec![base.clone()])).surface;
    assert_eq!(
        render(&doc(vec![base.clone(), grade.clone()])).surface,
        expected
    );
    grade["enabled"] = json!(true);
    grade["opacity"] = json!(0);
    assert_eq!(render(&doc(vec![base, grade])).surface, expected);
}
