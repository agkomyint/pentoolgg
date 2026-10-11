//! The existing fill geometry samples directly into unquantized working RGB.
use pentool::{
    color::{
        catalog,
        icc::Intent,
        value::{Destination, Evaluator, ProfileSet},
    },
    fill::{self, LinearFillOptions, LinearFillResult},
};
use serde_json::{json, Value};
use std::path::Path;
const DOCUMENT: &str = "target/working-fill.pen";
fn doc() -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":4,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"ramp","x":0,"y":0,"width":4,"height":1,"fill":{"kind":"linear","space":{"space":"srgb:linear"},"intent":"relative-colorimetric","stops":[{"position":0,"color":"#000"},{"position":1,"color":"#fff"}]}}]);
    raw
}
fn node(raw: &Value) -> &Value {
    &raw["pages"][0]["layers"][0]["nodes"][0]
}
fn paint(raw: &mut Value) -> &mut Value {
    &mut raw["pages"][0]["layers"][0]["nodes"][0]["fill"]
}
fn render(raw: &Value, intent: Intent, space: &str) -> anyhow::Result<LinearFillResult> {
    let profiles: Vec<_> = raw["color_management"]["profiles"]
        .as_object()
        .unwrap()
        .keys()
        .chain(
            raw["color_management"]["aliases"]
                .as_object()
                .unwrap()
                .keys(),
        )
        .map(String::as_str)
        .collect();
    let set = ProfileSet::load(raw, Path::new(DOCUMENT), profiles)?;
    let mut engine = Evaluator::new(&set, intent)?;
    fill::render_linear(
        raw,
        node(raw),
        Path::new(DOCUMENT),
        kurbo::Affine::IDENTITY,
        &LinearFillOptions {
            dimensions: (
                raw["pages"][0]["canvas"]["width"].as_u64().unwrap() as u32,
                raw["pages"][0]["canvas"]["height"].as_u64().unwrap() as u32,
            ),
            scale: 1.0,
            working: Destination::parse(&json!({"space":space}))?,
        },
        &mut engine,
    )
}
fn near(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
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
fn explicit_native_linear_ramp_retains_exact_working_samples_without_rgb8() {
    let raw = doc();
    let before = raw.clone();
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    for (x, pixel) in result.surface.pixels().iter().enumerate() {
        let t = (x as f64 + 0.5) / 4.0;
        near(pixel, &[t, t, t, 1.0]);
    }
    assert!(!result.conversion_clipped);
    assert_eq!(result.surface.byte_len(), 4 * 32);
    assert_eq!(raw, before);
}

#[test]
fn legacy_interpolation_keeps_its_domain_and_converts_before_coverage_accumulation() {
    for space in ["srgb", "linear"] {
        let mut raw = doc();
        paint(&mut raw)["space"] = json!(space);
        paint(&mut raw).as_object_mut().unwrap().remove("intent");
        let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
        for (x, pixel) in result.surface.pixels().iter().enumerate() {
            let t = if space == "linear" {
                (x as f64 + 0.5) / 4.0
            } else {
                (decode((x as f64 + 0.25) / 4.0) + decode((x as f64 + 0.75) / 4.0)) / 2.0
            };
            near(pixel, &[t, t, t, 1.0]);
        }
        assert!(!result.conversion_clipped);
    }
}

#[test]
fn native_straight_alpha_is_interpolated_before_premultiplied_coverage_average() {
    let mut raw = doc();
    paint(&mut raw)["stops"] = json!([
        {"position":0,"color":{"space":"srgb:linear","channels":[1,0,0],"alpha":0}},
        {"position":1,"color":{"space":"srgb:linear","channels":[0,0,1],"alpha":1}}
    ]);
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    for (x, pixel) in result.surface.pixels().iter().enumerate() {
        let a = (x as f64 + 0.25) / 4.0;
        let b = (x as f64 + 0.75) / 4.0;
        let alpha = (a + b) / 2.0;
        near(
            pixel,
            &[
                ((1.0 - a) * a + (1.0 - b) * b) / (2.0 * alpha),
                0.0,
                (a * a + b * b) / (2.0 * alpha),
                alpha,
            ],
        );
    }
}

#[test]
fn solid_native_paint_live_tokens_and_fractional_geometry_keep_precision() {
    let mut raw = doc();
    raw["styles"]["paint"] = json!({"type":"color","value":{"space":"srgb:linear","channels":[0.123456789,0.234567891,0.345678912],"alpha":0.456789123}});
    *paint(&mut raw) = json!({"kind":"solid","color":{"ref":"paint","fallback":"#f00"}});
    raw["pages"][0]["layers"][0]["nodes"][0]["x"] = json!(0.5);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(2.0);
    let before = raw.clone();
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    for (pixel, coverage) in result.surface.pixels().iter().zip([0.5, 1.0, 0.5, 0.0]) {
        if coverage == 0.0 {
            assert_eq!(*pixel, [0.0; 4]);
        } else {
            near(
                pixel,
                &[
                    0.123456789,
                    0.234567891,
                    0.345678912,
                    0.456789123 * coverage,
                ],
            );
        }
    }
    assert_eq!(raw, before);
}

#[test]
fn explicit_p3_gradient_retains_extended_working_rgb_but_legacy_domain_reports_clipping() {
    let mut raw = doc();
    paint(&mut raw)["space"] = json!({"space":"display-p3:linear"});
    paint(&mut raw)["stops"] = json!([
        {"position":0,"color":{"space":"display-p3:linear","channels":[1,0,0]}},
        {"position":1,"color":{"space":"display-p3:linear","channels":[1,0,0]}}
    ]);
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    assert!(result.surface.pixels()[0][0] > 1.22);
    assert!(result.surface.pixels()[0][1] < -0.04);
    assert!(result.surface.pixels()[0][2] < -0.019);
    assert!(!result.conversion_clipped);
    paint(&mut raw)["space"] = json!("srgb");
    paint(&mut raw).as_object_mut().unwrap().remove("intent");
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    near(&result.surface.pixels()[0], &[1.0, 0.0, 0.0, 1.0]);
    assert!(result.conversion_clipped);
}

#[test]
fn native_cmyk_ramp_uses_selected_intent_before_working_conversion() {
    let mut raw = doc();
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    paint(&mut raw)["space"] = json!({"space":"icc-cmyk","profile":"press"});
    paint(&mut raw)["stops"] = json!([
        {"position":0,"color":{"space":"icc-cmyk","profile":"press","channels":[0.1,0.2,0.3,0.4]}},
        {"position":1,"color":{"space":"icc-cmyk","profile":"press","channels":[0.8,0.7,0.6,0.5]}}
    ]);
    for intent in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ] {
        paint(&mut raw)["intent"] = serde_json::to_value(intent).unwrap();
        let result = render(&raw, intent, "srgb:linear").unwrap();
        let set = ProfileSet::load(&raw, Path::new(DOCUMENT), ["press"]).unwrap();
        let mut engine = Evaluator::new(&set, intent).unwrap();
        let destination = Destination::parse(&json!({"space":"srgb:linear"})).unwrap();
        for (x, pixel) in result.surface.pixels().iter().enumerate() {
            let mut expected = [0.0; 4];
            for offset in [0.25, 0.75] {
                let t = (x as f64 + offset) / 4.0;
                let color = pentool::color::value::NativeColor::parse(&json!({"space":"icc-cmyk","profile":"press","channels":[0.1*(1.0-t)+0.8*t,0.2*(1.0-t)+0.7*t,0.3*(1.0-t)+0.6*t,0.4*(1.0-t)+0.5*t]})).unwrap();
                let sample = engine.working(&color, &destination).unwrap();
                for (expected, sample) in expected.iter_mut().zip(sample.rgba) {
                    *expected += sample * 0.5;
                }
            }
            near(pixel, &expected);
        }
    }
}

#[test]
fn bounds_version_working_space_and_gradient_intent_are_checked_without_mutation() {
    let raw = doc();
    let before = raw.clone();
    assert!(render(&raw, Intent::Perceptual, "srgb:linear").is_err());
    assert!(render(&raw, Intent::RelativeColorimetric, "srgb").is_err());
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), std::iter::empty()).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    for (dimensions, scale) in [
        ((0, 1), 1.0),
        ((16384, 16384), 1.0),
        ((4, 1), f32::NAN),
        ((4, 1), 0.0),
    ] {
        let options = LinearFillOptions {
            dimensions,
            scale,
            working: Destination::parse(&json!({"space":"srgb:linear"})).unwrap(),
        };
        assert!(fill::render_linear(
            &raw,
            node(&raw),
            Path::new(DOCUMENT),
            kurbo::Affine::IDENTITY,
            &options,
            &mut engine
        )
        .is_err());
    }
    let mut old = raw.clone();
    old["version"] = json!(7);
    assert!(render(&old, Intent::RelativeColorimetric, "srgb:linear").is_err());
    assert_eq!(raw, before);
}

#[test]
fn physical_lab_ramp_matches_independent_cie_lightness_before_quantization() {
    let mut raw = doc();
    paint(&mut raw)["space"] = json!({"space":"lab-d50"});
    paint(&mut raw)["stops"] = json!([
        {"position":0,"color":{"space":"lab-d50","channels":[0,0,0]}},
        {"position":1,"color":{"space":"lab-d50","channels":[120,0,0]}}
    ]);
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    for (x, pixel) in result.surface.pixels().iter().enumerate() {
        let y = [0.25, 0.75]
            .into_iter()
            .map(|offset| {
                let l = 120.0 * (x as f64 + offset) / 4.0;
                if l > 8.0 {
                    ((l + 16.0) / 116.0).powi(3)
                } else {
                    l / (24389.0 / 27.0)
                }
            })
            .sum::<f64>()
            / 2.0;
        near(pixel, &[y, y, y, 1.0]);
    }
    assert!(result.surface.pixels()[3][0] > 1.0);
    assert!(!result.conversion_clipped);
}

#[test]
fn all_gradient_geometries_and_spreads_share_precise_coverage_samples() {
    for kind in ["linear", "radial", "conic"] {
        for spread in ["pad", "repeat", "reflect"] {
            let mut raw = doc();
            paint(&mut raw)["kind"] = json!(kind);
            paint(&mut raw)["spread"] = json!(spread);
            match kind {
                "linear" => {
                    paint(&mut raw)["x2"] = json!(0.5);
                }
                "radial" => {
                    paint(&mut raw)["cx"] = json!(0);
                    paint(&mut raw)["cy"] = json!(0);
                    paint(&mut raw)["radius"] = json!(0.5);
                }
                _ => {
                    paint(&mut raw)["cx"] = json!(2);
                    paint(&mut raw)["cy"] = json!(0.5);
                    paint(&mut raw)["angle"] = json!(45);
                }
            }
            let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
            for (x, pixel) in result.surface.pixels().iter().enumerate() {
                let mut expected = 0.0;
                for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let px = x as f64 + sx;
                    let t = match kind {
                        "linear" => px / 0.5,
                        "radial" => (px * px + sy * sy).sqrt() / 0.5,
                        _ => {
                            (f64::atan2(sy - 0.5, px - 2.0) - std::f64::consts::FRAC_PI_4)
                                .rem_euclid(std::f64::consts::TAU)
                                / std::f64::consts::TAU
                        }
                    };
                    expected += match spread {
                        "repeat" => t.rem_euclid(1.0),
                        "reflect" => {
                            let t = t.rem_euclid(2.0);
                            if t > 1.0 {
                                2.0 - t
                            } else {
                                t
                            }
                        }
                        _ => t.clamp(0.0, 1.0),
                    } * 0.25;
                }
                near(pixel, &[expected, expected, expected, 1.0]);
            }
        }
    }
}

#[test]
fn scale_and_parent_transform_use_the_existing_node_local_geometry() {
    let mut raw = doc();
    *paint(&mut raw) = json!({"kind":"solid","color":{"space":"srgb:linear","channels":[0.2,0.4,0.6],"alpha":0.123456789}});
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(1);
    let set = ProfileSet::load(&raw, Path::new(DOCUMENT), std::iter::empty()).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let options = LinearFillOptions {
        dimensions: (4, 2),
        scale: 2.0,
        working: Destination::parse(&json!({"space":"srgb:linear"})).unwrap(),
    };
    let result = fill::render_linear(
        &raw,
        node(&raw),
        Path::new(DOCUMENT),
        kurbo::Affine::translate((1.0, 0.0)),
        &options,
        &mut engine,
    )
    .unwrap();
    for row in result.surface.pixels().chunks_exact(4) {
        assert_eq!(row[0], [0.0; 4]);
        assert_eq!(row[1], [0.0; 4]);
        near(&row[2], &[0.2, 0.4, 0.6, 0.123456789]);
        near(&row[3], &row[2]);
    }
    let blank = fill::render_linear(
        &raw,
        node(&raw),
        Path::new(DOCUMENT),
        kurbo::Affine::scale(0.0),
        &options,
        &mut engine,
    )
    .unwrap();
    assert!(blank
        .surface
        .pixels()
        .iter()
        .all(|pixel| *pixel == [0.0; 4]));
}

#[test]
fn patterns_sample_into_working_rgb_with_srgb_interpretation() {
    let mut raw = doc();
    let image: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v5-image-embedded.pen")).unwrap();
    raw["image_assets"] = image["image_assets"].clone();
    let (identity, asset) = raw["image_assets"]
        .as_object()
        .unwrap()
        .iter()
        .next()
        .map(|(key, value)| (key.clone(), value.clone()))
        .unwrap();
    let data = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        asset["storage"]["data"].as_str().unwrap(),
    )
    .unwrap();
    let source = image::load_from_memory(&data).unwrap().to_rgba8();
    assert_eq!(source.dimensions(), (1, 1));
    let pixel = source.get_pixel(0, 0);
    let decode = |v: u8| {
        let v = f64::from(v) / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let expected = [
        decode(pixel[0]),
        decode(pixel[1]),
        decode(pixel[2]),
        f64::from(pixel[3]) / 255.0,
    ];
    *paint(&mut raw) = json!({"kind":"pattern","asset":identity});
    pentool::scene::validate(&raw).unwrap();
    let before = raw.clone();
    let result = render(&raw, Intent::RelativeColorimetric, "srgb:linear").unwrap();
    assert_eq!(raw, before);
    for pixel in result.surface.pixels() {
        assert!(pixel.iter().all(|v| v.is_finite()));
        near_loose(pixel, &expected);
    }
}
fn near_loose(actual: &[f64], expected: &[f64]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-6, "{actual:?} != {expected:?}");
    }
}
