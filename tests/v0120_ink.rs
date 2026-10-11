use pentool::{
    color::{
        catalog,
        icc::Intent,
        ink::{self, Thresholds},
        roles::{self, Role},
    },
    scene,
};
use serde_json::{json, Value};
use std::path::Path;

const PRESS: &[u8] = include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc");

fn document() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}

fn thresholds() -> Thresholds {
    Thresholds {
        tac_percent: 300.0,
        small_text_points: 8.0,
        units_per_inch: 72.0,
        neutral_chroma: 2.0,
    }
}

#[test]
fn requires_explicit_cmyk_output_and_valid_limits() {
    let raw = document();
    let path = Path::new("target/offline-ink.pen");
    let error = ink::analyze_page(&raw, path, None, thresholds(), 0, 10)
        .unwrap_err()
        .to_string();
    assert!(error.contains("CMYK output role"), "{error}");
    assert!(ink::analyze_page(&raw, path, None, thresholds(), 0, 0).is_err());
}

#[test]
fn profile_derived_paint_samples_are_bounded_and_paginated() {
    let path = Path::new("target/offline-ink.pen");
    let mut raw = document();
    let imported = catalog::import(&mut raw, path, PRESS, "press", "allowed", None, None).unwrap();
    let profile = imported["profile"].as_str().unwrap();
    roles::assign(
        &mut raw,
        path,
        Role::Output,
        &json!({"space":"icc-cmyk","profile":profile}),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-cmyk","profile":profile,"channels":[0.4,0.4,0.3,0.9]});
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"text","id":"fine-print","content":"print","x":2,"y":12,"font_family":"Arial","font_size":7,"style":{"fill":{"fallback":{"space":"icc-cmyk","profile":profile,"channels":[0.4,0.4,0.3,0.9]}}}}));
    scene::validate(&raw).unwrap();
    let before = raw.clone();
    let first = ink::analyze_page(&raw, path, Some("page-1"), thresholds(), 0, 2).unwrap();
    assert_eq!(first["scope"], "visible-paint-samples");
    assert_eq!(first["summary"]["sample_count"], 3);
    assert_eq!(first["returned"], 2);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["samples"][1]["channels"]["c"], 0.4);
    assert_eq!(first["samples"][1]["rich_black"], true);
    let tail = ink::analyze_page(&raw, path, Some("page-1"), thresholds(), 2, 2).unwrap();
    assert_eq!(tail["returned"], 1);
    assert_eq!(tail["has_more"], false);
    assert_eq!(tail["samples"][0]["text_points"], 7.0);
    assert_eq!(tail["samples"][0]["small_text_multi_plate"], true);
    assert_eq!(raw, before);

    let mut empty = raw.clone();
    empty["pages"][0]["canvas"]["background"] = json!("none");
    empty["pages"][0]["layers"][0]["nodes"] = json!([]);
    scene::validate(&empty).unwrap();
    let report = ink::analyze_page(&empty, path, None, thresholds(), 4, 2).unwrap();
    assert_eq!(report["summary"]["sample_count"], 0);
    assert_eq!(report["summary"]["channel_min"], Value::Null);
    assert_eq!(report["summary"]["channel_max"], Value::Null);
    assert_eq!(report["summary"]["maximum_tac_percent"], Value::Null);
    assert_eq!(report["returned"], 0);
    assert_eq!(report["has_more"], false);
}
