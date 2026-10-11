use pentool::{
    color::{catalog, proof},
    scene,
};
use serde_json::{json, Value};
use std::path::Path;

const PRESS: &[u8] = include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc");

fn document() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}

#[test]
fn proof_requires_an_explicit_output_condition() {
    let raw = document();
    let sample = json!({"space":"srgb","channels":[0.2,0.4,0.7]});
    let error = proof::evaluate_sample(&raw, Path::new("unused.pen"), &sample, false)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("proof color role is not configured"),
        "{error}"
    );
}

#[test]
fn proof_reports_device_round_trip_gamut_comparison_and_limits() {
    let mut raw = document();
    catalog::import(
        &mut raw,
        Path::new("unused.pen"),
        PRESS,
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let identity = catalog::resolve(&raw, "press").unwrap().0.to_owned();
    raw["color_management"]["roles"]["proof"] = json!({
        "destination":{"space":"icc-cmyk","profile":identity},
        "intent":"relative-colorimetric"
    });
    scene::validate(&raw).unwrap();

    let sample = json!({"space":"srgb","channels":[0.2,0.4,0.7],"alpha":0.35});
    let before = raw.clone();
    let report = proof::evaluate_sample(&raw, Path::new("unused.pen"), &sample, false).unwrap();
    assert_eq!(report["scope"], "soft-proof-sample");
    assert_eq!(report["proof"]["channels"]["space"], "icc-cmyk");
    assert_eq!(report["proof"]["numbers_preserved"], false);
    assert!(report["proof"]["round_trip_delta_e_76"]
        .as_f64()
        .unwrap()
        .is_finite());
    assert_eq!(report["proof"]["capabilities"]["ink_simulation"], true);
    assert_eq!(
        report["proof"]["capabilities"]["spectral_paper_or_ink_model"],
        false
    );
    assert_eq!(report["display"]["color"]["space"], "srgb");
    assert!(report["display"]["limitations"].as_array().unwrap().len() >= 3);
    assert_eq!(report["mutated"], false);
    assert_eq!(raw, before);

    let reports = proof::evaluate_samples(
        &raw,
        Path::new("unused.pen"),
        std::slice::from_ref(&sample),
        false,
    )
    .unwrap();
    assert_eq!(reports["matches"], 1);
    assert!(proof::evaluate_samples(
        &raw,
        Path::new("unused.pen"),
        &vec![sample; proof::MAX_PROOF_SAMPLES + 1],
        false
    )
    .unwrap_err()
    .to_string()
    .contains("limit-exceeded"));
}

#[test]
fn preserve_numbers_only_applies_to_the_identical_proof_device_profile() {
    let mut raw = document();
    catalog::import(
        &mut raw,
        Path::new("unused.pen"),
        PRESS,
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let identity = catalog::resolve(&raw, "press").unwrap().0.to_owned();
    raw["color_management"]["roles"]["proof"] = json!({
        "destination":{"space":"icc-cmyk","profile":identity},
        "intent":"relative-colorimetric"
    });
    let sample = json!({"space":"icc-cmyk","profile":"press","channels":[0.1,0.2,0.3,0.4]});
    let report = proof::evaluate_sample(&raw, Path::new("unused.pen"), &sample, true).unwrap();
    assert_eq!(report["proof"]["numbers_preserved"], true);
    assert_eq!(report["proof"]["channels"]["channels"], sample["channels"]);
}
