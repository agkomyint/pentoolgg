//! Structured print preflight: stable codes, severities and read-only behavior.
use pentool::color::{
    catalog, preflight,
    spot::{Ink, Library, Provenance},
};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/preflight.pen";

fn doc() -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
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
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"paint","x":0,"y":0,
        "width":200,"height":100,
        "fill":{"kind":"solid","color":{"space":"srgb","channels":[0.9,0.2,0.1]}}}]);
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":9});
    raw
}

fn codes(report: &Value) -> Vec<String> {
    report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["code"].as_str().unwrap().to_owned())
        .collect()
}

fn run(raw: &Value) -> Value {
    preflight::run(
        raw,
        Path::new(DOCUMENT),
        None,
        &preflight::Options::default(),
    )
    .unwrap()
}

#[test]
fn a_ready_page_passes_and_the_document_is_untouched() {
    let raw = doc();
    let before = raw.clone();
    let report = run(&raw);
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(
        (report["errors"].clone(), report["mutated"].clone()),
        (json!(0), json!(false))
    );
    assert_eq!(raw, before);
}

#[test]
fn missing_conditions_are_errors_with_corrective_actions() {
    let mut raw = doc();
    raw["color_management"]["roles"] = json!({});
    raw["pages"][0]
        .as_object_mut()
        .unwrap()
        .remove("production");
    let report = run(&raw);
    assert_eq!(report["ok"], false);
    let found = codes(&report);
    assert!(
        found.contains(&"output-role-missing".to_owned()),
        "{found:?}"
    );
    assert!(
        found.contains(&"production-missing".to_owned()),
        "{found:?}"
    );
    for issue in report["issues"].as_array().unwrap() {
        assert!(!issue["fix"].as_str().unwrap().is_empty());
    }
}

#[test]
fn unpainted_bleed_and_thin_bleed_are_reported() {
    let mut raw = doc();
    raw["pages"][0]["canvas"]["background"] = json!("#00000000");
    raw["pages"][0]["layers"][0]["nodes"][0]["x"] = json!(30);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(100);
    raw["pages"][0]["production"]["bleed"] = json!(3);
    let report = run(&raw);
    let found = codes(&report);
    assert!(found.contains(&"bleed-incomplete".to_owned()), "{found:?}");
    assert!(
        found.contains(&"bleed-below-minimum".to_owned()),
        "{found:?}"
    );
    // Skipping the render-based measurement drops only the coverage issue.
    let skipped = preflight::run(
        &raw,
        Path::new(DOCUMENT),
        None,
        &preflight::Options {
            bleed_scale: None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!codes(&skipped).contains(&"bleed-incomplete".to_owned()));
}

#[test]
fn transparency_text_and_spot_use_are_warned() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"][0]["opacity"] = json!(0.5);
    raw["pages"][0]["layers"][0]["nodes"][0]["style"] = json!({"overprint":{"fill":true}});
    let found = codes(&run(&raw));
    assert!(
        found.contains(&"transparency-flattened".to_owned()),
        "{found:?}"
    );
    assert!(
        found.contains(&"overprint-not-in-pdfx".to_owned()),
        "{found:?}"
    );
}

#[test]
fn invalid_options_and_versions_fail_before_work() {
    let raw = doc();
    let bad = preflight::run(
        &raw,
        Path::new(DOCUMENT),
        None,
        &preflight::Options {
            min_ppi: 0.0,
            min_bleed_inches: 0.125,
            bleed_scale: None,
            max_total_ink: None,
            allowed_spots: None,
            k_only_text: false,
            require_marks: false,
            reject_spot_flattening: false,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(bad.contains("minimum resolution"), "{bad}");
    let mut old = raw.clone();
    old["version"] = json!(4);
    let error = preflight::run(&old, Path::new(DOCUMENT), None, &Default::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported-version"), "{error}");
}

#[test]
fn total_ink_limit_is_measured_on_the_delivered_cmyk() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"rich","x":0,"y":0,
        "width":200,"height":100,
        "fill":{"kind":"solid","color":{"space":"icc-cmyk","profile":"press",
            "channels":[1.0,1.0,1.0,1.0]}}}]);
    let unlimited = run(&raw);
    assert!(!codes(&unlimited).contains(&"ink-total-exceeded".to_owned()));
    assert!(unlimited["max_total_ink"].is_null());
    let options = |limit: f64| preflight::Options {
        max_total_ink: Some(limit),
        bleed_scale: None,
        ..Default::default()
    };
    let strict = preflight::run(&raw, Path::new(DOCUMENT), None, &options(200.0)).unwrap();
    assert!(
        codes(&strict).contains(&"ink-total-exceeded".to_owned()),
        "{strict}"
    );
    assert_eq!(strict["ok"], false);
    assert_eq!(strict["max_total_ink"], 200.0);
    // A limit at the physical maximum never trips.
    let loose = preflight::run(&raw, Path::new(DOCUMENT), None, &options(400.0)).unwrap();
    assert!(!codes(&loose).contains(&"ink-total-exceeded".to_owned()));
    // Out-of-range limits fail before any rendering.
    for bad in [50.0, 500.0, f64::NAN] {
        let error = preflight::run(&raw, Path::new(DOCUMENT), None, &options(bad))
            .unwrap_err()
            .to_string();
        assert!(error.contains("total ink limit"), "{error}");
    }
}

#[test]
fn allowed_spots_make_unlisted_inks_an_error() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"rect","id":"dieline","x":0,"y":0,"width":50,"height":50,
         "style":{"fill":{"spot":{"ink":"dieline","tint":1.0},
                          "fallback":{"space":"srgb","channels":[1,0,0]}}}},
        {"kind":"rect","id":"gold","x":60,"y":0,"width":50,"height":50,
         "style":{"stroke":{"spot":{"ink":"gold","tint":0.5},
                            "fallback":{"space":"srgb","channels":[0.8,0.6,0.1]}}}}
    ]);
    let mut library = Library::default();
    for id in ["dieline", "gold"] {
        library
            .insert(Ink {
                id: id.into(),
                name: format!("Ink {id}"),
                aliases: vec![],
                alternate: json!({"space":"srgb","channels":[0.95,0.22,0.03]}),
                provenance: Provenance {
                    source: "test library".into(),
                    publisher: None,
                    revision: None,
                    license: None,
                },
                redistribution: "allowed".into(),
            })
            .unwrap();
    }
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    let run_with = |allowed: Option<Vec<String>>| {
        preflight::run(
            &raw,
            Path::new(DOCUMENT),
            None,
            &preflight::Options {
                bleed_scale: None,
                allowed_spots: allowed,
                ..Default::default()
            },
        )
        .unwrap()
    };
    // No list: every ink is allowed.
    assert!(!codes(&run_with(None)).contains(&"spot-ink-not-allowed".to_owned()));
    let report = run_with(Some(vec!["dieline".into()]));
    let issues: Vec<&Value> = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "spot-ink-not-allowed")
        .collect();
    assert_eq!(issues.len(), 1, "{report}");
    assert_eq!(issues[0]["node"], "gold");
    assert_eq!(issues[0]["severity"], "error");
    assert_eq!(report["ok"], false);
    assert_eq!(report["allowed_spots"], json!(["dieline"]));
    // Listing both inks clears the error.
    let clear = run_with(Some(vec!["dieline".into(), "gold".into()]));
    assert!(!codes(&clear).contains(&"spot-ink-not-allowed".to_owned()));
}

#[test]
fn k_only_text_policy_flags_rich_text_only() {
    let mut raw = doc();
    let text = |id: &str, y: u32, channels: [f64; 4]| {
        json!({"kind":"text","id":id,"x":0,"y":y,"width":100,"height":20,
            "content":"Hi","font_size":12,
            "style":{"fill":{"space":"icc-cmyk","profile":"press","channels":channels}}})
    };
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        text("rich", 0, [0.6, 0.4, 0.4, 1.0]),
        text("plain", 30, [0.0, 0.0, 0.0, 1.0]),
    ]);
    let run_with = |k_only_text: bool| {
        preflight::run(
            &raw,
            Path::new(DOCUMENT),
            None,
            &preflight::Options {
                bleed_scale: None,
                k_only_text,
                ..Default::default()
            },
        )
        .unwrap()
    };
    assert!(!codes(&run_with(false)).contains(&"text-not-k-only".to_owned()));
    let report = run_with(true);
    let hits: Vec<&Value> = report["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["code"] == "text-not-k-only")
        .collect();
    assert_eq!(hits.len(), 1, "{report}");
    assert_eq!(hits[0]["node"], "rich");
    assert_eq!(report["ok"], false);
    assert_eq!(report["k_only_text"], true);
}

#[test]
fn require_marks_policy_needs_declared_printers_marks() {
    let mut raw = doc();
    let run_with = |raw: &Value, require_marks: bool| {
        preflight::run(
            raw,
            Path::new(DOCUMENT),
            None,
            &preflight::Options {
                bleed_scale: None,
                require_marks,
                ..Default::default()
            },
        )
        .unwrap()
    };
    assert!(!codes(&run_with(&raw, false)).contains(&"marks-missing".to_owned()));
    let report = run_with(&raw, true);
    assert!(
        codes(&report).contains(&"marks-missing".to_owned()),
        "{report}"
    );
    assert_eq!(report["require_marks"], true);
    raw["pages"][0]["canvas"]["width"] = json!(240);
    raw["pages"][0]["canvas"]["height"] = json!(140);
    raw["pages"][0]["production"]["trim"] = json!({"x":40,"y":30,"width":160,"height":80});
    raw["pages"][0]["production"]["slug"] = json!({"top":20,"right":20,"bottom":20,"left":20});
    raw["pages"][0]["production"]["marks"] =
        json!({"crop":true,"registration":false,"offset":12,"length":6});
    assert!(!codes(&run_with(&raw, true)).contains(&"marks-missing".to_owned()));
}

#[test]
fn spot_policy_reject_makes_pdfx_flattening_an_error() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"rect","id":"dieline","x":0,"y":0,"width":50,"height":50,
         "style":{"fill":{"spot":{"ink":"dieline","tint":1.0},
                          "fallback":{"space":"srgb","channels":[1,0,0]}}}}
    ]);
    let mut library = Library::default();
    library
        .insert(Ink {
            id: "dieline".into(),
            name: "Ink dieline".into(),
            aliases: vec![],
            alternate: json!({"space":"srgb","channels":[0.95,0.22,0.03]}),
            provenance: Provenance {
                source: "test library".into(),
                publisher: None,
                revision: None,
                license: None,
            },
            redistribution: "allowed".into(),
        })
        .unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    let run_with = |reject_spot_flattening: bool| {
        preflight::run(
            &raw,
            Path::new(DOCUMENT),
            None,
            &preflight::Options {
                bleed_scale: None,
                reject_spot_flattening,
                ..Default::default()
            },
        )
        .unwrap()
    };
    let severity = |report: &Value| {
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["code"] == "spot-not-in-pdfx")
            .map(|i| i["severity"].as_str().unwrap().to_owned())
    };
    let lenient = run_with(false);
    assert_eq!(severity(&lenient).as_deref(), Some("warning"), "{lenient}");
    let strict = run_with(true);
    assert_eq!(severity(&strict).as_deref(), Some("error"), "{strict}");
    assert_eq!(strict["ok"], false);
    assert_eq!(strict["reject_spot_flattening"], true);
}
