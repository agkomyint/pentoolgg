//! Recipe spot substitution: paints of one ink are separated onto another's plate.
use pentool::color::{
    catalog, recipe,
    separation::{self, Options},
    spot::{Ink, Library, Provenance},
};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/spot-substitution.pen";

fn ink(id: &str) -> Ink {
    Ink {
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
    }
}

fn paint(id: &str, x: f64) -> Value {
    json!({"kind":"rect","id":format!("{id}-{x}"),"x":x,"y":0,"width":40,"height":40,
        "style":{"fill":{"spot":{"ink":id,"tint":1.0},
            "fallback":{"space":"srgb","channels":[0.95,0.22,0.03]}}}})
}

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
    raw["pages"][0]["layers"][0]["nodes"] = json!([paint("gold", 0.0), paint("silver", 50.0)]);
    let mut library = Library::default();
    for id in ["gold", "silver", "copper"] {
        library.insert(ink(id)).unwrap();
    }
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    raw
}

fn plates(raw: &Value) -> Vec<String> {
    separation::separate(
        raw,
        Path::new(DOCUMENT),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap()
    .plates
    .iter()
    .filter(|p| p.spot)
    .map(|p| p.id.clone())
    .collect()
}

#[test]
fn substituted_paints_land_on_the_target_plate_and_the_source_is_untouched() {
    let mut raw = doc();
    let original = raw.clone();
    assert_eq!(plates(&raw), ["gold", "silver"]);
    let parsed =
        recipe::parse(br#"{"version":1,"name":"one-spot","spot_substitutions":{"silver":"gold"}}"#)
            .unwrap();
    assert_eq!(
        parsed.spot_substitutions,
        [("silver".to_owned(), "gold".to_owned())]
    );
    assert_eq!(parsed.substitute_spots(&mut raw).unwrap(), 1);
    assert_eq!(plates(&raw), ["gold"]);
    // Only the ink reference changed; the tint, fallback and geometry stay.
    let node = &raw["pages"][0]["layers"][0]["nodes"][1];
    assert_eq!(
        node["style"]["fill"]["spot"],
        json!({"ink":"gold","tint":1.0})
    );
    assert_eq!(
        node["width"],
        original["pages"][0]["layers"][0]["nodes"][1]["width"]
    );
}

#[test]
fn unknown_inks_and_bad_maps_are_named_errors() {
    for (text, needle) in [
        (
            r#"{"version":1,"name":"a","spot_substitutions":{"gold":"gold"}}"#,
            "to itself",
        ),
        (
            r#"{"version":1,"name":"a","spot_substitutions":{"gold":"silver","silver":"copper"}}"#,
            "chains",
        ),
        (
            r#"{"version":1,"name":"a","spot_substitutions":{"gold":"bad id"}}"#,
            "spot_substitutions",
        ),
        (
            r#"{"version":1,"name":"a","spot_substitutions":[]}"#,
            "spot_substitutions",
        ),
    ] {
        let error = recipe::parse(text.as_bytes()).unwrap_err().to_string();
        assert!(error.contains(needle), "{text}: {error}");
    }
    let mut raw = doc();
    let before = raw.clone();
    let typo = recipe::parse(br#"{"version":1,"name":"a","spot_substitutions":{"silvr":"gold"}}"#)
        .unwrap();
    let error = typo.substitute_spots(&mut raw).unwrap_err().to_string();
    assert!(error.contains("unknown ink silvr"), "{error}");
    assert_eq!(raw, before);
}

#[test]
fn cli_recipe_substitutes_for_separation_and_preflight() {
    let dir = std::env::temp_dir().join(format!("pentool-spotsub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut raw = doc();
    raw["pages"][0]["production"] = json!({"units_per_inch":72,
        "trim":{"x":5,"y":5,"width":90,"height":40},"bleed":0});
    std::fs::write(dir.join("doc.pen"), serde_json::to_vec(&raw).unwrap()).unwrap();
    std::fs::write(
        dir.join("r.json"),
        br#"{"version":1,"name":"one-gold","spot_substitutions":{"silver":"gold"},"allowed_spots":["gold"]}"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        (
            output.status.success(),
            serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null),
        )
    };
    let (_, plain) = run(&["color", "separate", "doc.pen", "--out", "plain"]);
    let ids = |report: &Value| -> Vec<String> {
        report["plates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(ids(&plain).contains(&"silver".to_owned()), "{plain}");
    let (ok, merged) = run(&[
        "color", "separate", "doc.pen", "--out", "merged", "--recipe", "r.json",
    ]);
    assert!(ok, "{merged}");
    assert!(!ids(&merged).contains(&"silver".to_owned()), "{merged}");
    assert!(ids(&merged).contains(&"gold".to_owned()));
    // The allow-list names only gold, so substitution is what clears silver.
    let (_, report) = run(&[
        "color",
        "preflight",
        "doc.pen",
        "--bleed-scale",
        "0",
        "--recipe",
        "r.json",
    ]);
    assert!(
        !report["issues"]
            .to_string()
            .contains("spot-ink-not-allowed"),
        "{report}"
    );
    // The document on disk is never rewritten by substitution.
    let after: Value =
        serde_json::from_slice(&std::fs::read(dir.join("doc.pen")).unwrap()).unwrap();
    assert_eq!(
        after["pages"][0]["layers"][0]["nodes"][1]["style"]["fill"]["spot"]["ink"],
        "silver"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
