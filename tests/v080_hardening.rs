use pentool::{composite, scene};
use serde_json::json;
use std::path::Path;
#[test]
fn fractional_transform_coefficients_and_hashed_masks_round_trip_exactly() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"rect","id":"fractional","x":0.19586248838469542,"y":1,"width":2,"height":2,"transform":[0.9803826637651766,0.19586248838469542,-0.0893210546611558,1.149370692569433,-11.560239050186624,28.175101883420552],"style":{"fill":{"fallback":"#ffffff"}}}]);
    pentool::mask::create(
        &mut raw,
        Path::new("target/fractional.pen"),
        None,
        "stable",
        "vector:fractional",
    )
    .unwrap();
    let bytes = serde_json::to_vec(&raw).unwrap();
    let loaded: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(loaded, raw);
    scene::validate(&loaded).unwrap();
    assert_eq!(serde_json::to_vec(&loaded).unwrap(), bytes);
}

#[test]
fn development_schema_references_resolve_to_tracked_definitions() {
    fn walk(schema: &serde_json::Value, file: &Path) {
        if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
            let (relative, pointer) = reference.split_once('#').unwrap();
            let target = if relative.is_empty() {
                file.to_owned()
            } else {
                file.parent().unwrap().join(relative)
            };
            let data: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
            assert!(
                data.pointer(pointer).is_some(),
                "{}: {reference}",
                file.display()
            );
        }
        match schema {
            serde_json::Value::Object(fields) => {
                for child in fields.values() {
                    walk(child, file)
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    walk(child, file)
                }
            }
            _ => {}
        }
    }
    for file in [
        "docs/pen-format-v6.schema.json",
        "docs/penpreset.schema.json",
    ] {
        let path = Path::new(file);
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        walk(&data, path);
    }
}
#[test]
fn grouping_respects_target_locks_and_ungroup_keeps_compositing_atomic() {
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"group","id":"g","opacity":1.0,"content_opacity":1.0,"children":[{"kind":"rect","id":"r","x":0,"y":0,"width":2,"height":2,"style":{"fill":{"fallback":"#ffffff"}}}]}]);
    let plain = raw.clone();
    scene::apply_group(
        &mut raw,
        None,
        scene::GroupAction::Ungroup { id: "g".into() },
    )
    .unwrap();
    assert_eq!(raw["pages"][0]["layers"][0]["nodes"][0]["id"], "r");
    raw = plain;
    raw["pages"][0]["layers"][0]["nodes"][0]["opacity"] = json!(0.5);
    let before = raw.clone();
    assert!(scene::apply_group(
        &mut raw,
        None,
        scene::GroupAction::Ungroup { id: "g".into() }
    )
    .is_err());
    assert_eq!(raw, before);
    raw["pages"][0]["layers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"locked","name":"Locked","locked":true,"nodes":[]}));
    let before = raw.clone();
    assert!(scene::apply_group(
        &mut raw,
        None,
        scene::GroupAction::RemoveChild {
            group: "g".into(),
            child: "r".into(),
            layer: "locked".into()
        }
    )
    .is_err());
    assert_eq!(raw, before);
}
#[test]
fn tracked_composite_fixture_has_analytic_golden_pixels_on_every_target() {
    let path = Path::new("docs/fixtures/v6-composite.pen");
    let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let pixels = composite::render(&raw, path, Some("composite"), 1.0).unwrap();
    // Integer-aligned shapes have exact coverage. Encoded sRGB source-over then
    // 25% invert yields these four regions; no golden refresh is involved.
    for (x, y, pixel) in pixels.enumerate_pixels() {
        let expected = if x < 2 && y < 2 {
            [127, 96, 64, 255]
        } else if (2..4).contains(&x) && (2..4).contains(&y) {
            [136, 80, 88, 255]
        } else if (4..6).contains(&x) && (2..4).contains(&y) {
            [76, 88, 100, 255]
        } else {
            [80, 96, 112, 255]
        };
        assert_eq!(pixel.0, expected, "{x},{y}");
    }
    let transformed = composite::render(&raw, path, Some("transforms"), 1.0).unwrap();
    for (x, _, pixel) in transformed.enumerate_pixels() {
        let v = ((f64::from(x) + 0.5) / 8.0 * 255.0).round() as u8;
        assert_eq!(pixel.0, [v, v, v, 255]);
    }
}
#[test]
fn budgets_reject_before_allocating_large_surfaces() {
    let raw = composite::migrate(scene::new_document(16384, 16384)).unwrap();
    let error = composite::render(&raw, Path::new("target/limits.pen"), None, 1.0).unwrap_err();
    assert!(error.to_string().contains("limit-exceeded"));
    let mut raw = composite::migrate(scene::new_document(8, 8)).unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"solid","x":0,"y":0,"width":8,"height":8,"blend_mode":17,"fill":{"kind":"solid","color":"#ffffff"}}]);
    assert!(scene::validate(&raw).is_err());
    raw["pages"][0]["layers"][0]["nodes"][0]["blend_mode"] = json!("normal");
    raw["pages"][0]["layers"][0]["nodes"][0]["transforms"]=json!((0..65).map(|i|json!({"id":format!("op-{i}"),"kind":"scale","enabled":true,"opacity":1,"matrix":[1,0,0,0,1,0,0,0,1]})).collect::<Vec<_>>());
    assert!(scene::validate(&raw).is_err());
}
#[test]
fn composite_benchmark_cold_and_warm_are_identical() {
    let report = pentool::benchmark::run_composite(pentool::benchmark::CompositeBenchmark {
        clipped: 2,
        depth: 2,
        source_size: 16,
        blur: 2.0,
        repetitions: 1,
    })
    .unwrap();
    assert_eq!(
        report["runs"][0]["png_sha256"],
        report["runs"][1]["png_sha256"]
    );
}
