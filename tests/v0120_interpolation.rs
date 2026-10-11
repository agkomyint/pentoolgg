//! Explicit native-space interpolation preserves process channels and alpha.
use pentool::{
    color::{catalog, icc::Intent, value},
    resource,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

fn doc() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
fn sample(space: &str, channels: &[f64], alpha: f64) -> Value {
    json!({"space":space,"channels":channels,"alpha":alpha})
}
fn mix(raw: &Value, a: &Value, b: &Value, space: &Value, t: f64) -> anyhow::Result<Value> {
    value::interpolate(
        raw,
        Path::new("target/interpolation.pen"),
        a,
        b,
        space,
        t,
        Intent::RelativeColorimetric,
    )
}
fn near(value: &Value, expected: &[f64]) {
    for (v, e) in value.as_array().unwrap().iter().zip(expected) {
        assert!(
            (v.as_f64().unwrap() - e).abs() < 1e-12,
            "{value} != {expected:?}"
        );
    }
    assert_eq!(value.as_array().unwrap().len(), expected.len());
}

#[test]
fn interpolation_space_changes_rgb_midpoint_and_preserves_straight_alpha() {
    let raw = doc();
    let a = sample("srgb", &[0.0; 3], 0.0);
    let b = sample("srgb", &[1.0; 3], 1.0);
    let encoded = mix(&raw, &a, &b, &json!({"space":"srgb"}), 0.5).unwrap();
    let linear = mix(&raw, &a, &b, &json!({"space":"srgb:linear"}), 0.5).unwrap();
    near(&encoded["color"]["channels"], &[0.5; 3]);
    near(&linear["color"]["channels"], &[0.5; 3]);
    let preview = value::evaluate(
        &raw,
        Path::new("target/interpolation.pen"),
        &linear["color"],
        Some(&json!({"space":"srgb"})),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    near(
        &preview["converted"]["color"]["channels"],
        &[0.7353569830524495; 3],
    );
    assert_eq!(encoded["color"]["alpha"], 0.5);
    assert_eq!(linear["color"]["alpha"], 0.5);
    assert_eq!(encoded["alpha_interpolation"], "straight-separate");
    let transparent = mix(
        &raw,
        &sample("srgb", &[1.0, 0.0, 0.0], 0.0),
        &sample("srgb", &[0.0, 0.0, 1.0], 0.0),
        &json!({"space":"srgb"}),
        0.5,
    )
    .unwrap();
    near(&transparent["color"]["channels"], &[0.5, 0.0, 0.5]);
    assert_eq!(transparent["color"]["alpha"], 0.0);
}

#[test]
fn cmyk_aliases_interpolate_all_four_channels_without_reseparating_or_using_rgb() {
    let mut raw = doc();
    let bytes = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");
    catalog::import(
        &mut raw,
        Path::new("target/interpolation.pen"),
        bytes,
        "process",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let identity = resource::sha256(bytes);
    let mut a = sample("icc-cmyk", &[0.0, 0.2, 0.4, 1.0], 0.2);
    a["profile"] = json!("process");
    let mut b = sample("icc-cmyk", &[1.0, 0.8, 0.6, 0.0], 0.8);
    b["profile"] = json!(identity);
    let before = raw.clone();
    let target = json!({"space":"icc-cmyk","profile":"process"});
    let result = mix(&raw, &a, &b, &target, 0.25).unwrap();
    near(&result["color"]["channels"], &[0.25, 0.35, 0.45, 0.75]);
    assert!((result["color"]["alpha"].as_f64().unwrap() - 0.35).abs() < 1e-12);
    assert_eq!(result["endpoints"][0]["path"], "identity");
    assert_eq!(result["endpoints"][1]["path"], "identity");
    assert_eq!(result["profiles"], json!([identity]));
    assert_eq!(result["endpoint_clipped"], false);
    // The authored input profile has no reverse transform: this success proves
    // that interpolation did not silently reseparate through an RGB appearance.
    assert!(mix(&raw, &sample("srgb", &[0.1; 3], 1.0), &b, &target, 0.5).is_err());
    assert_eq!(raw, before);
}

#[test]
fn physical_lab_and_endpoints_retain_extended_channels_without_quantization() {
    let raw = doc();
    let a = sample("lab-d50", &[-10.0, 200.0, -180.0], 0.123456789);
    let b = sample("lab-d50", &[120.0, -140.0, 160.0], 0.987654321);
    let target = json!({"space":"lab-d50"});
    near(
        &mix(&raw, &a, &b, &target, 0.5).unwrap()["color"]["channels"],
        &[55.0, 30.0, -10.0],
    );
    assert_eq!(mix(&raw, &a, &b, &target, 0.0).unwrap()["color"], a);
    assert_eq!(mix(&raw, &a, &b, &target, 1.0).unwrap()["color"], b);
}

#[test]
fn mixed_sources_use_the_shared_conversion_and_report_endpoint_clipping() {
    let raw = doc();
    let a = sample("display-p3", &[1.0, 0.0, 0.0], 0.3);
    let b = sample("lab-d50", &[40.0, 10.0, 20.0], 0.7);
    let target = json!({"space":"srgb"});
    let result = mix(&raw, &a, &b, &target, 0.3).unwrap();
    for (i, source) in [&a, &b].into_iter().enumerate() {
        let converted = value::evaluate(
            &raw,
            Path::new("target/interpolation.pen"),
            source,
            Some(&target),
            Intent::RelativeColorimetric,
        )
        .unwrap();
        assert_eq!(result["endpoints"][i], converted["converted"]);
    }
    assert_eq!(result["endpoint_clipped"], true);
    assert_eq!(result["mutated"], false);
}

#[test]
fn invalid_positions_versions_headers_and_destinations_fail_explicitly() {
    let raw = doc();
    let color = sample("srgb", &[0.2; 3], 1.0);
    let target = json!({"space":"srgb"});
    for t in [-0.1, 1.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(mix(&raw, &color, &color, &target, t).is_err());
    }
    let mut old = raw.clone();
    old["version"] = json!(7);
    assert!(mix(&old, &color, &color, &target, 0.5).is_err());
    assert!(mix(
        &raw,
        &color,
        &color,
        &json!({"space":"srgb","channels":[0,0,0]}),
        0.5
    )
    .is_err());
    assert!(mix(
        &raw,
        &color,
        &color,
        &json!({"space":"icc-rgb","profile":"absent"}),
        0.5
    )
    .is_err());
    let mut raw = raw;
    let bytes = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
    catalog::import(
        &mut raw,
        Path::new("target/interpolation.pen"),
        bytes,
        "rgb",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let mut wrong = sample("icc-cmyk", &[0.2; 4], 1.0);
    wrong["profile"] = json!("rgb");
    assert!(mix(
        &raw,
        &wrong,
        &wrong,
        &json!({"space":"icc-cmyk","profile":"rgb"}),
        0.5
    )
    .is_err());
}

#[test]
fn interpolation_cli_is_bounded_read_only_and_rejects_page_scope() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/print-v8-native.pen");
    let before = std::fs::read(&path).unwrap();
    let from = r#"{"space":"srgb","channels":[0,0,0]}"#;
    let to = r#"{"space":"srgb","channels":[1,1,1]}"#;
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args([
                "color",
                "interpolate",
                path.to_str().unwrap(),
                "--from",
                from,
                "--to",
                to,
                "--space",
                r#"{"space":"srgb"}"#,
                "--position",
                "0.5",
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let output = run(&["--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    near(&report["color"]["channels"], &[0.5; 3]);
    assert!(!run(&["--page", "page-1"]).status.success());
    assert!(!run(&["--space", &"x".repeat(257)]).status.success());
    assert_eq!(std::fs::read(path).unwrap(), before);
}
