//! Slug text prints a job-information line in registration color in the top
//! slug, on every process plate, and is validated against the slug geometry.
use pentool::color::{
    production,
    separation::{self, Options},
};
use serde_json::{json, Value};
use std::path::Path;

fn doc(slug_text: Value, top: u32) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":400,"height":200,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!([]);
    let mut spec = json!({"units_per_inch":72,
        "trim":{"x":60,"y":60,"width":280,"height":80},"bleed":9,
        "slug":{"top":top,"right":20,"bottom":20,"left":20},
        "marks":{"crop":true,"registration":true,"offset":9,"length":12}});
    if !slug_text.is_null() {
        spec["slug_text"] = slug_text;
    }
    raw["pages"][0]["production"] = spec;
    raw
}

#[test]
fn slug_text_prints_only_in_the_top_slug_on_process_plates() {
    let raw = doc(json!("JOB 1234 PROOF"), 40);
    let result = separation::separate(
        &raw,
        Path::new("target/slug.pen"),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!(result.slug_text, 1);
    for id in ["C", "M", "Y", "K"] {
        let plate = result.plates.iter().find(|p| p.id == id).unwrap();
        let mut inked = 0;
        for y in 0..200 {
            for x in 0..400 {
                if plate.samples[y * 400 + x] > 0.0 && (11..38).contains(&y) && x >= 72 {
                    // Mark pixels start at y = 39; everything here is text.
                    inked += 1;
                }
            }
        }
        assert!(inked > 20, "{id} plate has no slug text: {inked}");
        let strong = plate.samples.iter().filter(|v| **v > 0.9).count();
        assert!(strong > 0);
        // Left of the text start and the page centre stay clear.
        assert_eq!(plate.samples[15 * 400 + 40], 0.0);
        assert_eq!(plate.samples[100 * 400 + 200], 0.0);
    }
    // Without the declaration nothing is painted in that band.
    let plain = separation::separate(
        &doc(Value::Null, 40),
        Path::new("target/slug.pen"),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!(plain.slug_text, 0);
    let k = plain.plates.iter().find(|p| p.id == "K").unwrap();
    assert!(k.samples[11 * 400..38 * 400].iter().all(|v| *v == 0.0));
}

#[test]
fn slug_text_is_validated_against_the_page() {
    let resolve = |raw: &Value| production::resolve(&raw["pages"][0], true);
    assert!(resolve(&doc(json!("ok"), 40)).is_ok());
    for (value, top, needle) in [
        (json!(""), 40, "printable ASCII"),
        (json!("caf\u{e9}"), 40, "printable ASCII"),
        (json!(7), 40, "must be a string"),
        (json!("x".repeat(201)), 40, "printable ASCII"),
        (json!("x".repeat(100)), 40, "too long"),
        (json!("ok"), 15, "increase slug.top"),
    ] {
        let err = resolve(&doc(value.clone(), top)).unwrap_err().to_string();
        assert!(err.contains(needle), "{value}: {err}");
    }
}
