//! Production marks are rendered in registration color: on the PDF/X raster
//! and on every process plate, and only where declared.
use pentool::color::{
    production,
    separation::{self, Options},
};
use serde_json::{json, Value};
use std::path::Path;

fn doc(marks: Value) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":200,"height":100,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!([]);
    let mut spec = json!({"units_per_inch":144,
        "trim":{"x":40,"y":20,"width":120,"height":60},"bleed":10,
        "slug":{"top":10,"right":10,"bottom":10,"left":10},"safe":6});
    if !marks.is_null() {
        spec["marks"] = marks;
    }
    raw["pages"][0]["production"] = spec;
    raw
}

fn marks() -> Value {
    json!({"crop":true,"registration":true,"offset":10,"length":10})
}

#[test]
fn painter_covers_mark_pixels_and_nothing_else() {
    let raw = doc(marks());
    let geometry = production::resolve(&raw["pages"][0], true)
        .unwrap()
        .unwrap();
    let mut grid = vec![0.0_f64; 200 * 100];
    let count = geometry.paint_marks(200, 100, |x, y, c| {
        let at = y as usize * 200 + x as usize;
        grid[at] = (grid[at] + c).min(1.0);
    });
    assert_eq!(count, 12);
    // The left crop mark runs along the top trim edge (y = 20) from x = 20..30.
    assert!(grid[19 * 200 + 25] + grid[20 * 200 + 25] >= 0.99);
    // The middle of the artwork and the far corner stay untouched.
    assert_eq!(grid[50 * 200 + 100], 0.0);
    assert_eq!(grid[0], 0.0);
    // Deterministic: a second pass paints exactly the same coverage.
    let mut again = vec![0.0_f64; 200 * 100];
    geometry.paint_marks(200, 100, |x, y, c| {
        let at = y as usize * 200 + x as usize;
        again[at] = (again[at] + c).min(1.0);
    });
    assert_eq!(grid, again);
    // No declared marks, no painting.
    let plain = doc(Value::Null);
    let geometry = production::resolve(&plain["pages"][0], true)
        .unwrap()
        .unwrap();
    assert_eq!(geometry.paint_marks(200, 100, |_, _, _| panic!()), 0);
}

#[test]
fn process_plates_carry_registration_marks() {
    let raw = doc(marks());
    let separations = separation::separate(
        &raw,
        Path::new("target/marks.pen"),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!(separations.marks, 12);
    for id in ["C", "M", "Y", "K"] {
        let plate = separations.plates.iter().find(|p| p.id == id).unwrap();
        let top = plate.samples[19 * 200 + 25].max(plate.samples[20 * 200 + 25]);
        assert!(top > 0.45, "{id} plate missed the crop mark: {top}");
        assert_eq!(plate.samples[50 * 200 + 100], 0.0);
    }
    assert_eq!(separations.json()["marks"], 12);
    let bare = separation::separate(
        &doc(Value::Null),
        Path::new("target/marks.pen"),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!(bare.marks, 0);
    assert!(bare
        .plates
        .iter()
        .all(|p| p.samples.iter().all(|v| *v == 0.0)));
}

#[test]
fn pdfx_raster_prints_marks_in_registration_color() {
    use pentool::color::{catalog, pdfx};
    let mut raw = doc(marks());
    catalog::import(
        &mut raw,
        Path::new("target/marks.pen"),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    let built = pdfx::build(
        &raw,
        Path::new("target/marks.pen"),
        None,
        &pdfx::Options {
            compress: true,
            ppi: 144.0,
            created: Some("2026-10-10T00:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(built.report["marks"]["rendered"], 12);
    assert_eq!(built.report["marks"]["ink"], "registration");
    pdfx::verify_structure(&built.bytes).unwrap();
    let mut without = doc(Value::Null);
    without["color_management"] = raw["color_management"].clone();
    let plain = pdfx::build(
        &without,
        Path::new("target/marks.pen"),
        None,
        &pdfx::Options {
            compress: true,
            ppi: 144.0,
            created: Some("2026-10-10T00:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plain.report["marks"]["rendered"], 0);
    assert_ne!(built.report["sha256"], plain.report["sha256"]);
}

#[test]
fn color_bars_print_known_patches_on_plates_and_are_bounded() {
    let mut raw = doc(json!({"color_bars":true,"offset":10,"length":10}));
    let separations = separation::separate(
        &raw,
        Path::new("target/marks.pen"),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!(separations.color_bar_patches, 8);
    assert_eq!(separations.marks, 0);
    // Patch 0 is solid cyan at x 40..45, y 90..100 (bottom slug, offset 10).
    let value = |plate: &str, x: usize, y: usize| {
        separations
            .plates
            .iter()
            .find(|p| p.id == plate)
            .unwrap()
            .samples[y * 200 + x]
    };
    assert_eq!(value("C", 42, 95), 1.0);
    assert_eq!(value("K", 42, 95), 0.0);
    // Patch 7 is 50% black at x 75..80.
    assert_eq!(value("K", 77, 95), 0.5);
    assert_eq!(value("C", 77, 95), 0.0);
    // A trim too narrow for the bar is refused with the needed width.
    raw["pages"][0]["production"]["trim"] = json!({"x":40,"y":20,"width":40,"height":60});
    raw["pages"][0]["production"]["bleed"] = json!(10);
    let error = production::resolve(&raw["pages"][0], true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("color bar needs trim width"), "{error}");
}
