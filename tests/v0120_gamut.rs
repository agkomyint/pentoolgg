//! Gamut warning image: out-of-gamut artwork is red, in-gamut artwork is muted.
use pentool::color::{catalog, icc::Intent, proofimage};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/gamut.pen";

fn doc() -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":100,"height":50,"background":"#ffffffff"});
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
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"fill","id":"vivid","x":0,"y":0,"width":50,"height":50,
         "fill":{"kind":"solid","color":{"space":"srgb","channels":[0.0,1.0,0.0]}}},
        {"kind":"fill","id":"gray","x":50,"y":0,"width":50,"height":50,
         "fill":{"kind":"solid","color":{"space":"srgb","channels":[0.5,0.5,0.5]}}}
    ]);
    raw
}

#[test]
fn saturated_green_is_flagged_and_neutral_gray_is_not() {
    let raw = doc();
    let before = raw.clone();
    let result = proofimage::gamut(&raw, Path::new(DOCUMENT), None).unwrap();
    assert_eq!(raw, before);
    assert_eq!(
        result.report["gamut_warning_method"],
        "display-vs-soft-proof-round-trip-difference"
    );
    assert!(result.report["gamut_warning_pixels"].as_u64().unwrap() > 0);
    let picture = image::load_from_memory(&result.png).unwrap().to_rgb8();
    let (w, h) = picture.dimensions();
    assert_eq!(picture.get_pixel(w / 4, h / 2).0, [220, 38, 38]);
    assert_ne!(picture.get_pixel(3 * w / 4, h / 2).0, [220, 38, 38]);
    let plain = proofimage::render(&raw, Path::new(DOCUMENT), None).unwrap();
    assert!(plain.report["gamut_warning_pixels"].is_null());
}

#[test]
fn proof_setup_intent_overrides_the_role_and_is_reported() {
    let raw = doc();
    let relative = proofimage::render(&raw, Path::new(DOCUMENT), None).unwrap();
    assert_eq!(relative.report["intent"], "relative-colorimetric");
    let perceptual =
        proofimage::render_intent(&raw, Path::new(DOCUMENT), None, Some(Intent::Perceptual))
            .unwrap();
    assert_eq!(perceptual.report["intent"], "perceptual");
    let flagged =
        proofimage::gamut_intent(&raw, Path::new(DOCUMENT), None, Some(Intent::Perceptual))
            .unwrap();
    assert_eq!(flagged.report["intent"], "perceptual");
}
