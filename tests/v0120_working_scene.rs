//! Float scenes use the shared ordered compositor and preserve declared blends.
use pentool::{
    blend,
    color::{
        catalog,
        icc::Intent,
        value::{Destination, Evaluator, NativeColor, ProfileSet},
    },
    composite, mask,
};
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/working-scene.pen";
fn doc(nodes: Vec<Value>) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":1,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw
}
fn paint(id: &str, channels: [f64; 3], alpha: f64) -> Value {
    json!({"kind":"fill","id":id,"x":0,"y":0,"width":1,"height":1,
        "fill":{"kind":"solid","color":{"space":"srgb:linear","channels":channels,"alpha":alpha}}})
}
fn render(raw: &Value) -> anyhow::Result<composite::WorkingRender> {
    composite::render_working(raw, Path::new(DOCUMENT), None, 1.0, false)
}
fn near(actual: &[f64], expected: &[f64]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-12, "{actual:?} != {expected:?}");
    }
}
fn decode(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn stored_encoded_and_linear_blend_spaces_keep_distinct_meaning() {
    for space in ["srgb", "linear"] {
        let mut source = paint("source", [1.0; 3], 1.0);
        source["opacity"] = json!(0.5);
        source["blend_space"] = json!(space);
        let raw = doc(vec![paint("base", [0.0; 3], 1.0), source]);
        let before = raw.clone();
        let result = render(&raw).unwrap();
        let expected = if space == "srgb" { decode(0.5) } else { 0.5 };
        near(
            &result.surface.pixels()[0],
            &[expected, expected, expected, 1.0],
        );
        assert_eq!(result.working_space.space(), "srgb:linear");
        assert_eq!(result.peak_temporary_bytes, 104);
        assert!(result.pixel_work > 0);
        assert_eq!(raw, before);
    }
}

#[test]
fn repeated_low_opacity_layers_retain_float_coverage_and_color() {
    let mut nodes = vec![paint("base", [0.0; 3], 1.0)];
    for i in 0..1000 {
        let mut node = paint(&format!("paint-{i}"), [1.0; 3], 1.0);
        node["opacity"] = json!(0.0001);
        node["blend_space"] = json!("linear");
        nodes.push(node);
    }
    let raw = doc(nodes);
    let result = render(&raw).unwrap();
    let expected = 1.0 - 0.9999_f64.powi(1000);
    near(
        &result.surface.pixels()[0],
        &[expected, expected, expected, 1.0],
    );
    assert_eq!(
        composite::render(&raw, Path::new(DOCUMENT), None, 1.0)
            .unwrap()
            .get_pixel(0, 0)
            .0,
        [0, 0, 0, 255]
    );
}

#[test]
fn consecutive_clipping_uses_precise_base_coverage() {
    let alpha = 0.123456789;
    let mut blue = paint("blue", [0.0, 0.0, 1.0], 0.456789123);
    blue["clipping"] = json!({"base":"base"});
    blue["blend_space"] = json!("linear");
    let mut green = paint("green", [0.0, 1.0, 0.0], 0.7);
    green["clipping"] = json!({"base":"base"});
    green["blend_space"] = json!("linear");
    let raw = doc(vec![paint("base", [1.0, 0.0, 0.0], alpha), blue, green]);
    let result = render(&raw).unwrap();
    let mut expected =
        blend::LinearSurface::from_pixels(1, 1, "srgb:linear", vec![[1.0, 0.0, 0.0, alpha]])
            .unwrap();
    for source in [
        [0.0, 0.0, 1.0, alpha * 0.456789123],
        [0.0, 1.0, 0.0, alpha * 0.7],
    ] {
        expected
            .over(
                &blend::LinearSurface::from_pixels(1, 1, "srgb:linear", vec![source]).unwrap(),
                "normal",
            )
            .unwrap();
    }
    near(&result.surface.pixels()[0], &expected.pixels()[0]);
}

#[test]
fn isolated_and_pass_through_groups_share_existing_stack_semantics() {
    for isolation in ["isolated", "pass-through"] {
        let mut child = paint("inside", [0.5; 3], 1.0);
        child["blend_mode"] = json!("multiply");
        child["blend_space"] = json!("linear");
        let raw = doc(vec![
            paint("base", [0.2, 0.4, 0.6], 1.0),
            json!({"kind":"group","id":"group","isolation":isolation,"children":[child]}),
        ]);
        let result = render(&raw).unwrap();
        near(
            &result.surface.pixels()[0],
            if isolation == "isolated" {
                &[0.5, 0.5, 0.5, 1.0]
            } else {
                &[0.1, 0.2, 0.3, 1.0]
            },
        );
    }
}

#[test]
fn working_primaries_change_storage_without_reinterpreting_declared_srgb_blending() {
    let mut source = paint("source", [1.0; 3], 1.0);
    source["opacity"] = json!(0.5);
    let mut raw = doc(vec![paint("base", [1.0, 0.0, 0.0], 1.0), source]);
    raw["color_management"]["roles"] = json!({"working":{"destination":{"space":"display-p3:linear"},"intent":"relative-colorimetric"}});
    let result = render(&raw).unwrap();
    assert_eq!(result.surface.space(), "display-p3:linear");
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), std::iter::empty()).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let expected = engine
        .working(
            &NativeColor::parse(&json!({"space":"srgb","channels":[1,0.5,0.5]})).unwrap(),
            &Destination::parse(&json!({"space":"display-p3:linear"})).unwrap(),
        )
        .unwrap();
    near(&result.surface.pixels()[0], &expected.rgba);
    assert!(!result.conversion_clipped);
}

#[test]
fn native_cmyk_solid_consumes_working_intent_and_reports_paths_without_rgb8() {
    let mut raw = doc(vec![paint("ink", [0.0; 3], 0.234567891)]);
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["fill"]["color"] = json!({"space":"icc-cmyk","profile":"press","channels":[0.2,0.3,0.4,0.5],"alpha":0.234567891});
    assert!(render(&raw).is_err());
    raw["color_management"]["roles"] =
        json!({"working":{"destination":{"space":"srgb:linear"},"intent":"perceptual"}});
    let before = raw.clone();
    let result = render(&raw).unwrap();
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), ["press"]).unwrap();
    let mut engine = Evaluator::new(&set, Intent::Perceptual).unwrap();
    let expected = engine
        .working(
            &NativeColor::parse(&raw["pages"][0]["layers"][0]["nodes"][0]["fill"]["color"])
                .unwrap(),
            &Destination::parse(&json!({"space":"srgb:linear"})).unwrap(),
        )
        .unwrap();
    near(&result.surface.pixels()[0], &expected.rgba);
    assert_eq!(result.intent, Intent::Perceptual);
    assert_eq!(raw, before);
}

#[test]
fn float_limits_fail_explicitly_without_source_mutation() {
    let mut raw = doc(vec![paint("paint", [0.2; 3], 1.0)]);
    raw["pages"][0]["canvas"]["width"] = json!(4096);
    raw["pages"][0]["canvas"]["height"] = json!(4096);
    let before = raw.clone();
    let error = render(&raw).err().unwrap().to_string();
    assert!(error.contains("compositing exceeds temporary"), "{error}");
    assert_eq!(raw, before);
}

#[test]
fn accumulated_fill_transform_overflow_fails_in_both_precisions() {
    let mut node = paint("paint", [0.2; 3], 1.0);
    node["transform"] = json!([1e200, 0, 0, 1e200, 0, 0]);
    let raw = doc(vec![node]);
    pentool::scene::validate(&raw).unwrap();
    let before = raw.clone();
    assert!(render(&raw)
        .err()
        .unwrap()
        .to_string()
        .contains("transform overflow"));
    assert!(composite::render(&raw, Path::new(DOCUMENT), None, 1.0)
        .err()
        .unwrap()
        .to_string()
        .contains("transform overflow"));
    assert_eq!(raw, before);
}

#[test]
fn declared_blend_context_rejects_mismatched_primaries_and_late_extended_blends_atomically() {
    let mut destination =
        blend::LinearSurface::from_pixels(2, 1, "srgb:linear", vec![[0.2; 4], [0.2; 4]]).unwrap();
    let source = blend::LinearSurface::from_pixels(
        2,
        1,
        "srgb:linear",
        vec![[0.4; 4], [1.4, 0.2, 0.3, 0.5]],
    )
    .unwrap();
    let before = destination.clone();
    let context = blend::LegacyBlendContext::new("srgb:linear").unwrap();
    assert!(destination
        .over_declared(&source, "color-dodge", "linear", &context)
        .is_err());
    assert_eq!(destination, before);
    let other = blend::LegacyBlendContext::new("display-p3:linear").unwrap();
    assert!(destination
        .over_declared(&source, "normal", "srgb", &other)
        .is_err());
    assert_eq!(destination, before);
}

#[test]
fn srgb8_image_pixels_enter_working_storage_before_scene_blending() {
    let image: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v5-image-embedded.pen")).unwrap();
    let mut raw = catalog::migrate(image).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":1,"height":1,"background":"#00000000"});
    let node = &mut raw["pages"][0]["layers"][0]["nodes"][0];
    node["x"] = json!(0);
    node["y"] = json!(0);
    node["width"] = json!(1);
    node["height"] = json!(1);
    let before = raw.clone();
    let preview = composite::render(&raw, Path::new(DOCUMENT), None, 1.0).unwrap();
    let result = render(&raw).unwrap();
    let pixel = preview.get_pixel(0, 0);
    let expected = [
        decode(f64::from(pixel[0]) / 255.0),
        decode(f64::from(pixel[1]) / 255.0),
        decode(f64::from(pixel[2]) / 255.0),
        f64::from(pixel[3]) / 255.0,
    ];
    near(&result.surface.pixels()[0], &expected);
    assert!(result.peak_temporary_bytes >= 68);
    assert_eq!(raw, before);
}

#[test]
fn reusable_masks_multiply_float_coverage_without_quantizing_color() {
    let mut raw = doc(vec![paint("paint", [0.123456789, 0.4, 0.8], 0.8)]);
    raw["pages"][0]["canvas"]["width"] = json!(2);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(2);
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(
            json!({"kind":"rect","id":"mask-source","x":0,"y":0,"width":1,"height":1,
            "style":{"fill":{"fallback":"#ffffff"}}}),
        );
    mask::create(
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
    mask::attach(
        &mut raw,
        None,
        "paint",
        "half",
        &json!({"linked":false,"density":0.5}),
    )
    .unwrap();
    let before = raw.clone();
    let result = render(&raw).unwrap();
    near(&result.surface.pixels()[0], &[0.123456789, 0.4, 0.8, 0.8]);
    near(
        &result.surface.pixels()[1],
        &[0.123456789, 0.4, 0.8, 0.8 * 128.0 / 255.0],
    );
    assert_eq!(raw, before);
}

#[test]
fn native_vector_fill_and_stroke_colors_bypass_rgb8_preview_lowering() {
    let fill = json!({"space":"srgb:linear","channels":[0.123456789,0.4,0.8],"alpha":0.234567891});
    let stroke =
        json!({"space":"srgb:linear","channels":[0.8,0.314159265,0.2],"alpha":0.765432109});
    let mut raw = doc(vec![
        json!({"kind":"rect","id":"shape","x":0,"y":0,"width":1,"height":1,
        "style":{"fill":{"ref":"brand","fallback":"#000000"},"stroke":{"fallback":"none"},"stroke_width":{"fallback":0}}}),
    ]);
    raw["styles"] = json!({"brand":{"type":"color","value":fill}});
    let result = render(&raw).unwrap();
    near(
        &result.surface.pixels()[0],
        &[0.123456789, 0.4, 0.8, 0.234567891],
    );

    raw["pages"][0]["canvas"]["width"] = json!(3);
    raw["pages"][0]["canvas"]["height"] = json!(3);
    raw["pages"][0]["layers"][0]["nodes"][0] = json!({"kind":"rect","id":"stroke","x":1,"y":1,"width":1,"height":1,
        "style":{"fill":{"fallback":"none"},"stroke":{"fallback":stroke},"stroke_width":{"fallback":1}}});
    let result = render(&raw).unwrap();
    let covered: Vec<_> = result
        .surface
        .pixels()
        .iter()
        .filter(|pixel| pixel[3] > 0.0)
        .collect();
    assert!(!covered.is_empty());
    for pixel in covered {
        near(&pixel[..3], &[0.8, 0.314159265, 0.2]);
        assert!(pixel[3] <= 0.765432109);
    }
}

#[test]
fn text_geometry_uses_native_working_color_and_retains_antialiased_coverage() {
    let color = [0.234567891, 0.456789123, 0.678912345];
    let mut raw = doc(vec![
        json!({"kind":"text","id":"copy","content":"I","x":1,"y":14,
        "font_family":"sans-serif","font_size":14,"font_weight":400,
        "style":{"fill":{"fallback":{"space":"srgb:linear","channels":color,"alpha":0.625}}}}),
    ]);
    raw["pages"][0]["canvas"] = json!({"width":16,"height":16,"background":"#00000000"});
    let result = render(&raw).unwrap();
    let covered: Vec<_> = result
        .surface
        .pixels()
        .iter()
        .filter(|pixel| pixel[3] > 0.0)
        .collect();
    assert!(!covered.is_empty());
    assert!(covered.iter().any(|pixel| pixel[3] < 0.625));
    for pixel in covered {
        near(&pixel[..3], &color);
        assert!(pixel[3] <= 0.625);
    }
}
