//! Print recipes: strict parsing, bounded size, and flag-over-recipe CLI behavior.
use pentool::color::recipe;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

#[test]
fn recipes_parse_strictly() {
    let ok = recipe::parse(
        br#"{"version":1,"name":"magazine-print","pdf_standard":"PDF/X-4",
        "intent":"perceptual","ppi":300,"min_ppi":200,"min_bleed_inches":0.25,"bleed_scale":2}"#,
    )
    .unwrap();
    assert_eq!(ok.name, "magazine-print");
    assert_eq!(ok.min_ppi, Some(200.0));
    assert_eq!(ok.min_bleed_inches, Some(0.25));
    assert_eq!(ok.bleed_scale, Some(2.0));
    for (text, needle) in [
        (r#"{"version":2,"name":"a"}"#, "version must be 1"),
        (r#"{"version":1}"#, "name"),
        (r#"{"version":1,"name":"a b"}"#, "name"),
        (
            r#"{"version":1,"name":"a","typo":1}"#,
            "unknown print recipe field",
        ),
        (r#"{"version":1,"name":"a","ppi":5}"#, "ppi"),
        (
            r#"{"version":1,"name":"a","pdf_standard":"PDF/X-1a"}"#,
            "not supported",
        ),
        (r#"[]"#, "JSON object"),
    ] {
        let error = recipe::parse(text.as_bytes()).unwrap_err().to_string();
        assert!(error.contains(needle), "{text}: {error}");
    }
    let big = format!(r#"{{"version":1,"name":"a","x":"{}"}}"#, "z".repeat(20_000));
    assert!(recipe::parse(big.as_bytes())
        .unwrap_err()
        .to_string()
        .contains("limited"));
}

fn run(args: &[&str]) -> (bool, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    (
        out.status.success(),
        serde_json::from_str(&text).unwrap_or(Value::Null),
    )
}

#[test]
fn recipe_thresholds_drive_preflight_and_flags_override_them() {
    let dir = std::env::temp_dir().join(format!("pentool-recipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#ffffffff"});
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":20,"y":10,"width":160,"height":80},"bleed":9});
    let doc = dir.join("a.pen");
    std::fs::write(&doc, serde_json::to_vec(&raw).unwrap()).unwrap();
    let recipe_path = dir.join("r.json");
    std::fs::write(
        &recipe_path,
        br#"{"version":1,"name":"strict","min_bleed_inches":0.5,"bleed_scale":0}"#,
    )
    .unwrap();
    let doc_s = doc.to_str().unwrap();
    let recipe_s = recipe_path.to_str().unwrap();
    let codes = |report: &Value| -> Vec<String> {
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["code"].as_str().unwrap().to_owned())
            .collect()
    };
    // 9 units at 72/inch is 0.125 inch: fine by default, thin under the recipe.
    let (_, plain) = run(&["color", "preflight", doc_s, "--bleed-scale", "0"]);
    assert!(!codes(&plain).contains(&"bleed-below-minimum".to_owned()));
    let (_, strict) = run(&["color", "preflight", doc_s, "--recipe", recipe_s]);
    assert!(
        codes(&strict).contains(&"bleed-below-minimum".to_owned()),
        "{strict}"
    );
    let (_, flagged) = run(&[
        "color",
        "preflight",
        doc_s,
        "--recipe",
        recipe_s,
        "--min-bleed-inches",
        "0.1",
    ]);
    assert!(!codes(&flagged).contains(&"bleed-below-minimum".to_owned()));
    let bad = dir.join("bad.json");
    std::fs::write(&bad, br#"{"version":1,"name":"x","bogus":1}"#).unwrap();
    let (ok, _) = run(&[
        "color",
        "preflight",
        doc_s,
        "--recipe",
        bad.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(Path::new(doc_s).exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn max_total_ink_is_range_checked() {
    use pentool::color::recipe;
    let ok = recipe::parse(br#"{"version":1,"name":"ink","max_total_ink":300}"#).unwrap();
    assert_eq!(ok.max_total_ink, Some(300.0));
    for bad in ["50", "401", "\"300\"", "null"] {
        let text = format!(r#"{{"version":1,"name":"ink","max_total_ink":{bad}}}"#);
        assert!(recipe::parse(text.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn recipe_profile_names_a_cmyk_destination_and_needs_an_intent() {
    let ok = recipe::parse(
        br#"{"version":1,"name":"press","intent":"relative-colorimetric","profile":"press"}"#,
    )
    .unwrap();
    assert_eq!(ok.profile.as_deref(), Some("press"));
    let destination = ok.destination().unwrap();
    assert_eq!(destination.space(), "icc-cmyk");
    assert_eq!(destination.profile(), Some("press"));
    let error = recipe::parse(br#"{"version":1,"name":"press","profile":"press"}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("must also name an intent"), "{error}");
    for bad in [r#""""#, r#"7"#, r#""a\u0007b""#] {
        let text = format!(r#"{{"version":1,"name":"x","intent":"perceptual","profile":{bad}}}"#);
        assert!(recipe::parse(text.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn plate_bits_recipe_field_is_validated() {
    let ok = recipe::parse(br#"{"version":1,"name":"plates","plate_bits":16}"#).unwrap();
    assert_eq!(ok.plate_bits, Some(16));
    let bad = recipe::parse(br#"{"version":1,"name":"plates","plate_bits":12}"#).unwrap_err();
    assert!(bad.to_string().contains("plate_bits must be 8 or 16"));
}

#[test]
fn auto_trap_recipe_field_is_validated() {
    let ok = recipe::parse(br#"{"version":1,"name":"trap","auto_trap":0.25}"#).unwrap();
    assert_eq!(ok.auto_trap, Some(0.25));
    for bad in ["0", "-1", "\"wide\"", "5000"] {
        let text = format!(r#"{{"version":1,"name":"trap","auto_trap":{bad}}}"#);
        let error = recipe::parse(text.as_bytes()).unwrap_err();
        assert!(
            error.to_string().contains("auto_trap must be"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn scale_recipe_field_is_validated() {
    let ok = recipe::parse(br#"{"version":1,"name":"scale","scale":2}"#).unwrap();
    assert_eq!(ok.scale, Some(2.0));
    for bad in ["0", "9", "\"big\""] {
        let text = format!(r#"{{"version":1,"name":"scale","scale":{bad}}}"#);
        let error = recipe::parse(text.as_bytes()).unwrap_err();
        assert!(
            error.to_string().contains("scale must be"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn allowed_spots_recipe_field_is_validated() {
    let ok = recipe::parse(br#"{"version":1,"name":"spots","allowed_spots":["dieline","gold"]}"#)
        .unwrap();
    assert_eq!(
        ok.allowed_spots,
        Some(vec!["dieline".to_owned(), "gold".to_owned()])
    );
    for bad in [
        r#""dieline""#,
        r#"[1]"#,
        r#"[""]"#,
        r#"["a b"]"#,
        r#"["a","a"]"#,
    ] {
        let text = format!(r#"{{"version":1,"name":"spots","allowed_spots":{bad}}}"#);
        let error = recipe::parse(text.as_bytes()).unwrap_err();
        assert!(
            error.to_string().contains("allowed_spots"),
            "{bad}: {error}"
        );
    }
}

#[test]
fn black_policy_recipe_field_is_validated() {
    assert!(
        recipe::parse(br#"{"version":1,"name":"b","black_policy":"k-only-text"}"#)
            .unwrap()
            .k_only_text
    );
    assert!(
        !recipe::parse(br#"{"version":1,"name":"b","black_policy":"any"}"#)
            .unwrap()
            .k_only_text
    );
    let error = recipe::parse(br#"{"version":1,"name":"b","black_policy":"rich"}"#).unwrap_err();
    assert!(
        error.to_string().contains("black_policy must be"),
        "{error}"
    );
}

#[test]
fn require_marks_recipe_field_is_validated() {
    let ok = recipe::parse(br#"{"version":1,"name":"m","require_marks":true}"#).unwrap();
    assert!(ok.require_marks);
    let error = recipe::parse(br#"{"version":1,"name":"m","require_marks":"yes"}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("require_marks"), "{error}");
}

#[test]
fn embedded_recipes_resolve_by_name_through_the_cli() {
    let mut raw: serde_json::Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["print_recipes"] = serde_json::json!([
        {"version":1,"name":"strict","max_total_ink":300,"min_ppi":72}
    ]);
    assert_eq!(
        recipe::embedded(&raw, "strict").unwrap().max_total_ink,
        Some(300.0)
    );
    let missing = recipe::embedded(&raw, "other").unwrap_err().to_string();
    assert!(missing.contains("[missing-resource]"), "{missing}");

    let dir = std::env::temp_dir().join(format!("pentool-embedded-recipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("doc.pen"), serde_json::to_vec(&raw).unwrap()).unwrap();
    let before = std::fs::read(dir.join("doc.pen")).unwrap();
    let run = |name: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&dir)
            .args(["color", "preflight", "doc.pen", "--recipe", name])
            .output()
            .unwrap()
    };
    let output = run("@strict");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!("{}", String::from_utf8_lossy(&output.stderr));
    });
    assert_eq!(report["max_total_ink"], 300.0, "{report}");
    let bad = run("@nope");
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("[missing-resource]"));
    assert_eq!(std::fs::read(dir.join("doc.pen")).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compression_recipe_field_is_validated() {
    let none = recipe::parse(br#"{"version":1,"name":"c","compression":"none"}"#).unwrap();
    assert_eq!(none.compress, Some(false));
    let error = recipe::parse(br#"{"version":1,"name":"c","compression":"lzw"}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("compression"), "{error}");
}

#[test]
fn spot_policy_recipe_field_is_validated() {
    let reject = recipe::parse(br#"{"version":1,"name":"s","spot_policy":"reject"}"#).unwrap();
    assert!(reject.reject_spot_flattening);
    let error = recipe::parse(br#"{"version":1,"name":"s","spot_policy":"drop"}"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("spot_policy"), "{error}");
}
