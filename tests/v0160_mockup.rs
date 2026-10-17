use serde_json::Value;
use std::process::Command;

fn scratch() -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pentool-v0160-mockup-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn pentool(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn mockup_cli_lists_and_applies_starters_through_the_shared_form_transaction() {
    let list = pentool(&["mockup", "list", "--json"]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let catalog: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(catalog["format"], "pentool-mockup-catalog-v1");
    assert_eq!(catalog["presets"].as_array().unwrap().len(), 8);
    let preset_ids = catalog["presets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|preset| preset["id"].as_str())
        .collect::<Vec<_>>();
    for required in [
        "shipping-carton",
        "cylinder-container",
        "cosmetic-tube",
        "perfume-bottle",
        "cosmetic-jar",
        "stand-up-pouch",
        "product-card",
        "device-frame",
    ] {
        assert!(
            preset_ids.contains(&required),
            "missing preset {required}: {preset_ids:?}"
        );
    }

    let dir = scratch();
    let input = dir.join("product.pen");
    std::fs::write(
        &input,
        include_bytes!("../docs/fixtures/v11-product-packaging.pen"),
    )
    .unwrap();
    let path = input.to_str().unwrap();
    let before = std::fs::read(&input).unwrap();
    let dry_run = pentool(&[
        "mockup",
        "apply",
        path,
        "--preset",
        "perfume-bottle",
        "--id",
        "bottle-front",
        "--layer",
        "mockups",
        "--page",
        "products",
        "--width",
        "420",
        "--height",
        "620",
        "--dry-run",
        "--json",
    ]);
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let dry_report: Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(dry_report["id"], "bottle-front");
    assert_eq!(dry_report["product_form"], "bottle-front-form");
    assert_eq!(dry_report["change"]["dry_run"], true);

    let applied = pentool(&[
        "mockup",
        "apply",
        path,
        "--preset",
        "perfume-bottle",
        "--id",
        "bottle-front",
        "--layer",
        "mockups",
        "--page",
        "products",
        "--width",
        "420",
        "--height",
        "620",
        "--json",
    ]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let apply_report: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(dry_report["report"], apply_report["report"]);
    let document: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert!(document["product_forms"]["bottle-front-form"].is_object());
    assert!(document["pages"][1]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["id"] == "bottle-front" && node["product_form"] == "bottle-front-form"));

    let before_default = std::fs::read(&input).unwrap();
    let default_dry = pentool(&[
        "mockup",
        "apply",
        path,
        "--preset",
        "cosmetic-jar",
        "--id",
        "default-jar",
        "--layer",
        "mockups",
        "--page",
        "products",
        "--dry-run",
    ]);
    assert!(
        default_dry.status.success(),
        "{}",
        String::from_utf8_lossy(&default_dry.stderr)
    );
    assert_eq!(
        std::fs::read(&input).unwrap(),
        before_default,
        "dry run must not publish"
    );
    let default_preview: Value = serde_json::from_slice(&default_dry.stdout).unwrap();
    assert!(default_preview["report"]["vertices"].as_u64().unwrap() > 0);
    let default_apply = pentool(&[
        "mockup",
        "apply",
        path,
        "--preset",
        "cosmetic-jar",
        "--id",
        "default-jar",
        "--layer",
        "mockups",
        "--page",
        "products",
    ]);
    assert!(
        default_apply.status.success(),
        "{}",
        String::from_utf8_lossy(&default_apply.stderr)
    );
    let default_report: Value = serde_json::from_slice(&default_apply.stdout).unwrap();
    assert_eq!(default_preview["report"], default_report["report"]);
    let default_doc: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let default_node = default_doc["pages"][1]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "default-jar")
        .unwrap();
    assert_eq!(default_node["x"], 0.0);
    assert_eq!(default_node["y"], 0.0);
    assert_eq!(default_node["width"], 400.0);
    assert_eq!(default_node["height"], 400.0);

    let stale = pentool(&[
        "mockup",
        "apply",
        path,
        "--preset",
        "perfume-bottle",
        "--id",
        "stale-bottle",
        "--layer",
        "mockups",
        "--page",
        "products",
        "--if-revision",
        "sha256:stale",
    ]);
    assert!(!stale.status.success());
    assert_eq!(
        default_doc,
        serde_json::from_slice::<Value>(&std::fs::read(&input).unwrap()).unwrap()
    );

    let rendered = pentool(&["scene3d", path, "render-report", "bottle-front"]);
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let report: Value = serde_json::from_slice(&rendered.stdout).unwrap();
    assert_eq!(report["render"]["output"]["format"], "png");
    assert_eq!(report["render"]["output"]["width_px"], 420);
}

#[test]
fn mockup_cli_can_apply_every_listed_starter() {
    let dir = scratch();
    let input = dir.join("all-starters.pen");
    std::fs::write(
        &input,
        include_bytes!("../docs/fixtures/v11-product-packaging.pen"),
    )
    .unwrap();
    let path = input.to_str().unwrap();
    let initial: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let initial_form_count = initial["product_forms"].as_object().unwrap().len();
    let initial_scene3d_count = initial["pages"][1]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["kind"] == "scene3d")
        .count();
    let catalog = pentool(&["mockup", "list"]);
    assert!(
        catalog.status.success(),
        "{}",
        String::from_utf8_lossy(&catalog.stderr)
    );
    let catalog: Value = serde_json::from_slice(&catalog.stdout).unwrap();
    let presets = catalog["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 8);

    for preset in presets {
        let preset_id = preset["id"].as_str().unwrap();
        let node_id = format!("starter-{preset_id}");
        let output = pentool(&[
            "mockup", "apply", path, "--preset", preset_id, "--id", &node_id, "--layer", "mockups",
            "--page", "products",
        ]);
        assert!(
            output.status.success(),
            "preset {preset_id} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["id"], node_id);
        assert_eq!(report["product_form"], format!("{node_id}-form"));
        assert!(report["report"]["vertices"].as_u64().unwrap() > 0);
        assert!(report["report"]["triangles"].as_u64().unwrap() > 0);
    }

    let document: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        document["product_forms"].as_object().unwrap().len(),
        initial_form_count + 8
    );
    let nodes = document["pages"][1]["layers"][0]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node["kind"] == "scene3d")
            .count(),
        initial_scene3d_count + 8
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn procedural_preset_uv_rectangles_match_generated_material_faces() {
    use pentool::scene3d::form;

    for preset in form::starter_presets() {
        let id = preset["id"].as_str().unwrap();
        assert_eq!(
            preset["mockup"]["source"]["kind"], "procedural-recipe",
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["source"]["license"], "MIT OR Apache-2.0",
            "{id}"
        );
        assert!(
            preset["mockup"]["source"]["geometry_digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["uv"]["orientation"],
            "per-material region; u increases right, v increases up",
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["allowed_source_binding"]["kinds"],
            serde_json::json!(["page", "node"]),
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["allowed_source_binding"]["modes"],
            serde_json::json!(["decal", "fill"]),
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["camera"]["projection"], "perspective",
            "{id}"
        );
        assert_eq!(
            preset["mockup"]["camera"]["framing"], "automatic model bounds",
            "{id}"
        );
        assert!(
            preset["mockup"]["default_lighting"].as_str().is_some(),
            "{id}"
        );
        assert!(
            !preset["mockup"]["ground"]["enabled_by_default"]
                .as_bool()
                .unwrap(),
            "{id}"
        );
        assert!(
            preset["mockup"]["ground"]["optional"].as_bool().unwrap(),
            "{id}"
        );
        assert!(
            preset["mockup"]["shadows"]["enabled_by_default"]
                .as_bool()
                .unwrap(),
            "{id}"
        );
        assert!(
            preset["mockup"]["shadows"]["optional"].as_bool().unwrap(),
            "{id}"
        );
        let minimum_resolution = preset["mockup"]["minimum_useful_resolution_px"]
            .as_array()
            .unwrap();
        assert_eq!(minimum_resolution.len(), 2, "{id}");
        assert!(
            minimum_resolution
                .iter()
                .all(|edge| edge.as_u64().is_some_and(|edge| edge >= 640)),
            "{id}"
        );
        let value = form::starter_preset(id).unwrap();
        let model = form::generate(&value).unwrap_or_else(|error| panic!("{id}: {error:#}"));
        let slots = preset["material_slots"].as_array().unwrap();
        let regions = preset["mockup"]["uv"]["regions"].as_object().unwrap();
        for slot in preset["material_slots"].as_array().unwrap() {
            assert!(
                regions.contains_key(slot.as_str().unwrap()),
                "{id}: missing UV rectangle for {slot}"
            );
        }

        for slot in slots {
            let slot = slot.as_str().unwrap();
            let material_index = model
                .materials
                .iter()
                .position(|material| material.name == slot)
                .unwrap_or_else(|| panic!("{id}: declared slot {slot} has no material"));
            let matching = model
                .primitives
                .iter()
                .filter(|primitive| primitive.material == Some(material_index))
                .collect::<Vec<_>>();
            assert!(
                !matching.is_empty(),
                "{id}: declared slot {slot} has no faces"
            );

            let mut bounds = [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ];
            for primitive in matching {
                let uvs = primitive
                    .uvs
                    .first()
                    .unwrap_or_else(|| panic!("{id}/{slot}: UV0 missing"));
                for index in &primitive.indices {
                    let uv = uvs[*index as usize];
                    assert!(
                        uv[0].is_finite() && uv[1].is_finite(),
                        "{id}/{slot}: non-finite UV"
                    );
                    bounds[0] = bounds[0].min(uv[0]);
                    bounds[1] = bounds[1].min(uv[1]);
                    bounds[2] = bounds[2].max(uv[0]);
                    bounds[3] = bounds[3].max(uv[1]);
                }
            }
            let region = regions[slot].as_array().unwrap();
            let expected = [
                region[0].as_f64().unwrap() as f32,
                region[1].as_f64().unwrap() as f32,
                (region[0].as_f64().unwrap() + region[2].as_f64().unwrap()) as f32,
                (region[1].as_f64().unwrap() + region[3].as_f64().unwrap()) as f32,
            ];
            for axis in 0..4 {
                assert!(
                    (bounds[axis] - expected[axis]).abs() < 1e-5,
                    "{id}/{slot}: generated UV bound {} differs from catalog rectangle {}",
                    bounds[axis],
                    expected[axis]
                );
            }
        }
    }

    let mut misaligned = form::starter_preset("perfume-bottle").unwrap();
    misaligned["parameters"]["label_band_mm"] = serde_json::json!([22.0, 105.0]);
    let error = form::generate(&misaligned).unwrap_err();
    assert!(error.to_string().contains("align with profile rings"));
}
