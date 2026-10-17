use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen")).unwrap()
}

#[test]
fn bundled_product_starters_generate_bounded_parametric_meshes() {
    let presets = pentool::scene3d::form::starter_presets();
    assert_eq!(presets.len(), 8);
    for preset in presets {
        let id = preset["id"].as_str().unwrap();
        let metadata = &preset["mockup"];
        assert_eq!(metadata["source"]["kind"], "procedural-recipe", "{id}");
        assert!(
            metadata["source"]["geometry_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:"),
            "{id}"
        );
        assert_eq!(metadata["source"]["license"], "MIT OR Apache-2.0");
        assert!(metadata["uv"]["regions"].is_object(), "{id}");
        assert_eq!(
            metadata["allowed_source_binding"]["kinds"],
            serde_json::json!(["page", "node"])
        );
        assert_eq!(
            metadata["allowed_source_binding"]["modes"],
            serde_json::json!(["decal", "fill"])
        );
        assert!(metadata["camera"]["azimuth"].is_number(), "{id}");
        assert!(metadata["minimum_useful_resolution_px"].is_array(), "{id}");
        let form = pentool::scene3d::form::starter_preset(id).unwrap();
        assert!(
            form.get("mockup").is_none(),
            "catalog-only setup must not leak into form records"
        );
        let model = pentool::scene3d::form::generate(&form).unwrap();
        assert!(model.report.vertices > 0, "{id}");
        assert!(model.report.vertices <= pentool::scene3d::model::LIMITS.max_vertices);
        assert!(model.report.triangles > 0, "{id}");
        assert_eq!(
            model.materials.len(),
            form["material_slots"].as_array().unwrap().len(),
            "{id} must preserve its named editable regions"
        );
        assert!(form["parameters"].is_object());
    }
}

#[test]
fn stand_up_pouch_has_editable_regions_and_renders_deterministically() {
    let form = pentool::scene3d::form::starter_preset("stand-up-pouch").unwrap();
    let model = pentool::scene3d::form::generate(&form).unwrap();
    assert_eq!(model.materials.len(), 4);
    assert_eq!(model.primitives.len(), 8);
    let names: std::collections::BTreeSet<_> = model
        .primitives
        .iter()
        .map(|primitive| primitive.name.as_str())
        .collect();
    assert!(names.contains("pouch-front"));
    assert!(names.contains("pouch-back"));
    assert!(names.contains("pouch-gusset-right-front"));
    assert!(names.contains("pouch-gusset-left-back"));
    assert!(names.contains("pouch-top-seal"));
    assert!(model.primitives.iter().all(|primitive| {
        primitive.positions.len() == primitive.normals.len()
            && primitive.positions.len() == primitive.uvs[0].len()
            && primitive
                .positions
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            && primitive.indices.len() == 6
    }));
    for primitive in &model.primitives {
        let [a, b, c] = [
            primitive.positions[primitive.indices[0] as usize],
            primitive.positions[primitive.indices[1] as usize],
            primitive.positions[primitive.indices[2] as usize],
        ];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let normal = primitive.normals[0];
        let orientation = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2];
        assert!(
            orientation > 0.0,
            "{} has inconsistent winding/normals",
            primitive.name
        );
    }

    let mut doc = fixture();
    doc["product_forms"]["test-pouch"] = form;
    let node = serde_json::json!({
        "kind":"scene3d", "id":"pouch-preview", "x":20,"y":20,
        "width":180,"height":260,"product_form":"test-pouch"
    });
    doc["pages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|page| page["id"] == "products")
        .unwrap()["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(node.clone());
    let first = pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        node.as_object().unwrap(),
    )
    .unwrap();
    let second = pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        node.as_object().unwrap(),
    )
    .unwrap();
    assert_eq!(first.0, second.0);
    assert_eq!(second.1["cache_hit"], true);

    let mut grounded_doc = doc.clone();
    let grounded_node = serde_json::json!({
        "kind":"scene3d", "id":"pouch-preview", "x":20,"y":20,
        "width":180,"height":260,"product_form":"test-pouch",
        "ground_plane":{"enabled":true,"color":"#ddd8d0","size_multiplier":2.5}
    });
    grounded_doc["pages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|page| page["id"] == "products")
        .unwrap()["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|candidate| candidate["id"] == "pouch-preview")
        .unwrap()
        .clone_from(&grounded_node);
    let grounded = pentool::scene3d::node::render_png_report_in_page(
        &grounded_doc,
        std::path::Path::new("."),
        Some("products"),
        grounded_node.as_object().unwrap(),
    )
    .unwrap();
    assert_ne!(
        first.0, grounded.0,
        "the explicit ground plane must affect the rendered pixels"
    );
    assert_eq!(
        grounded.1["cache_hit"], false,
        "ground settings are part of cache identity"
    );
    assert_eq!(grounded.1["ground_plane"]["enabled"], true);

    let mut catcher_node = grounded_node.clone();
    catcher_node["ground_plane"]["mode"] = serde_json::json!("shadow-catcher");
    catcher_node["ground_plane"]["color"] = serde_json::json!("#000000");
    let catcher = pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        catcher_node.as_object().unwrap(),
    )
    .unwrap();
    let catcher_pixels = tiny_skia::Pixmap::decode_png(&catcher.0).unwrap();
    let alphas: Vec<u8> = catcher_pixels
        .data()
        .chunks_exact(4)
        .map(|pixel| pixel[3])
        .collect();
    assert!(
        alphas.contains(&0),
        "shadow catcher must preserve transparent background"
    );
    assert!(
        alphas.iter().any(|alpha| *alpha > 0 && *alpha < 255),
        "shadow catcher must emit translucent shadow pixels"
    );
    assert_ne!(catcher.0, grounded.0);

    let mut legacy = grounded_doc.clone();
    legacy["version"] = serde_json::json!(10);
    assert!(pentool::scene3d::node::render_png_report_in_page(
        &legacy,
        std::path::Path::new("."),
        Some("products"),
        grounded_node.as_object().unwrap(),
    )
    .unwrap_err()
    .to_string()
    .contains("require document format v11"));
    let mut out_of_bounds = grounded_node.clone();
    out_of_bounds["ground_plane"]["size_multiplier"] = serde_json::json!(4.1);
    assert!(pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        out_of_bounds.as_object().unwrap(),
    )
    .unwrap_err()
    .to_string()
    .contains("size_multiplier"));
    let mut invalid_mode = grounded_node.clone();
    invalid_mode["ground_plane"]["mode"] = serde_json::json!("reflective");
    assert!(pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        invalid_mode.as_object().unwrap()
    )
    .unwrap_err()
    .to_string()
    .contains("ground_plane.mode"));
    let mut translucent = grounded_node.clone();
    translucent["ground_plane"]["color"] = serde_json::json!("#d8d5d080");
    assert!(pentool::scene3d::node::render_png_report_in_page(
        &doc,
        std::path::Path::new("."),
        Some("products"),
        translucent.as_object().unwrap(),
    )
    .unwrap_err()
    .to_string()
    .contains("must be opaque"));

    let invalid = serde_json::json!({"kind":"pouch","generator":1,
        "material_slots":["front","back","gusset","seals"],
        "parameters":{"width_mm":100,"height_mm":200,"gusset_mm":100}});
    assert!(pentool::scene3d::form::generate(&invalid)
        .unwrap_err()
        .to_string()
        .contains("gusset_mm must be less than pouch width_mm"));
}

#[test]
fn bottle_profile_control_points_generate_bounded_smooth_curve_rings() {
    let form = serde_json::json!({
        "kind":"lathe-bottle","generator":1,
        "material_slots":["body"],
        "parameters":{
            "height_mm":100,"radial_segments":24,
            "profile_mm":[[12,0],[20,20],[18,70],[8,100]]
        }
    });
    let model = pentool::scene3d::form::generate(&form).unwrap();
    let repeated = pentool::scene3d::form::generate(&form).unwrap();
    let stride = 25;
    let points: Vec<_> = model.primitives[0]
        .positions
        .chunks_exact(stride)
        .map(|ring| {
            let point = ring[0];
            ((point[0].hypot(point[2])) as f64, point[1] as f64)
        })
        .collect();
    assert_eq!(
        points.len(),
        13,
        "four samples per authored segment plus the first ring"
    );
    assert_eq!(
        model.primitives[0].positions,
        repeated.primitives[0].positions
    );
    assert_eq!(
        model.primitives[0].uvs, repeated.primitives[0].uvs,
        "the default UV layout must be deterministic"
    );
    assert_eq!(points.first(), Some(&(12.0, 0.0)));
    assert_eq!(points.last(), Some(&(8.0, 100.0)));
    assert!(points
        .iter()
        .all(|(radius, height)| *radius > 0.0 && (0.0..=100.0).contains(height)));
    assert!(points.windows(2).all(|pair| pair[1].1 > pair[0].1));
    assert!(
        (points[1].0 - 14.0).abs() > 0.1,
        "intermediate rings should follow the curved control profile rather than linear chords"
    );

    let mut edited_form = form.clone();
    edited_form["parameters"]["profile_mm"][2][1] = serde_json::json!(75);
    let edited = pentool::scene3d::form::generate(&edited_form).unwrap();
    let edited_again = pentool::scene3d::form::generate(&edited_form).unwrap();
    assert_eq!(
        edited.primitives[0].uvs, edited_again.primitives[0].uvs,
        "the regenerated default UV layout must remain deterministic"
    );
    assert_ne!(
        model.primitives[0].uvs, edited.primitives[0].uvs,
        "moving a profile control point must update the generated surface UVs"
    );
}

#[test]
fn bottle_profile_tangents_are_bounded_deterministic_and_affect_curve() {
    let mut form = serde_json::json!({
        "kind":"lathe-bottle","generator":1,
        "material_slots":["body"],
        "parameters":{
            "height_mm":100,"radial_segments":24,
            "profile_mm":[[12,0],[20,20],[18,70],[8,100]]
        }
    });
    let automatic = pentool::scene3d::form::generate(&form).unwrap();
    form["parameters"]["profile_tangents"] = serde_json::json!([0.0, 0.0, 0.0, 0.0]);
    let explicit = pentool::scene3d::form::generate(&form).unwrap();
    let repeated = pentool::scene3d::form::generate(&form).unwrap();
    assert_ne!(
        automatic.primitives[0].positions,
        explicit.primitives[0].positions
    );
    assert_eq!(
        explicit.primitives[0].positions,
        repeated.primitives[0].positions
    );

    form["parameters"]["profile_tangents"] = serde_json::json!([0.0, 0.0]);
    assert!(pentool::scene3d::form::generate(&form)
        .unwrap_err()
        .to_string()
        .contains("one slope per profile point"));
    form["parameters"]["profile_tangents"] = serde_json::json!([0.0, 0.0, 20.1, 0.0]);
    assert!(pentool::scene3d::form::generate(&form)
        .unwrap_err()
        .to_string()
        .contains("within -20..=20"));
}

#[test]
fn lathe_bottle_can_generate_an_independent_bounded_cap_region() {
    let form = serde_json::json!({
        "kind":"lathe-bottle","generator":1,
        "material_slots":["body","cap"],
        "parameters":{
            "height_mm":180,"radial_segments":32,
            "profile_mm":[[12,0],[12,120],[8,150],[8,180]],
            "cap_mm":{"radius_mm":9,"height_mm":18}
        }
    });
    let model = pentool::scene3d::form::generate(&form).unwrap();
    let cap_parts: Vec<_> = model
        .primitives
        .iter()
        .filter(|primitive| primitive.name.starts_with("bottle-cap-"))
        .collect();
    assert_eq!(cap_parts.len(), 2);
    assert!(cap_parts.iter().all(|part| part.material == Some(1)));
    assert!(model
        .instances
        .iter()
        .any(|instance| instance.id == "bottle-cap-skirt"));
    assert!(model
        .instances
        .iter()
        .any(|instance| instance.id == "bottle-cap-crown"));

    let missing_slot = serde_json::json!({
        "kind":"lathe-bottle","generator":1,
        "parameters":form["parameters"]
    });
    assert!(pentool::scene3d::form::generate(&missing_slot)
        .unwrap_err()
        .to_string()
        .contains("requires the `cap` material slot"));
    let too_small = serde_json::json!({
        "kind":"lathe-bottle","generator":1,
        "material_slots":["body","cap"],
        "parameters":{
            "height_mm":180,"radial_segments":32,
            "profile_mm":[[12,0],[12,120],[8,150],[8,180]],
            "cap_mm":{"radius_mm":7,"height_mm":18}
        }
    });
    assert!(pentool::scene3d::form::generate(&too_small).is_err());
}

#[test]
fn bottle_cap_offset_moves_only_cap_geometry_and_defaults_for_legacy_forms() {
    let make = |cap| {
        serde_json::json!({
            "kind":"lathe-bottle","generator":1,
            "material_slots":["body","cap"],
            "parameters":{
                "height_mm":180,"radial_segments":32,
                "profile_mm":[[12,0],[12,120],[8,150],[8,180]],
                "cap_mm":cap
            }
        })
    };
    let legacy = make(serde_json::json!({"radius_mm":9,"height_mm":18}));
    let base = pentool::scene3d::form::generate(&legacy).unwrap();
    let shifted = pentool::scene3d::form::generate(&make(serde_json::json!({
        "radius_mm":9,"height_mm":18,"offset_mm":12
    })))
    .unwrap();

    assert_eq!(
        base.primitives[0].positions,
        shifted.primitives[0].positions
    );
    assert_eq!(base.primitives[0].uvs, shifted.primitives[0].uvs);
    for primitive_index in [1, 2] {
        let before = &base.primitives[primitive_index];
        let after = &shifted.primitives[primitive_index];
        assert_eq!(before.material, after.material);
        assert_eq!(before.uvs, after.uvs, "cap UVs remain local and stable");
        assert_eq!(before.positions.len(), after.positions.len());
        for (before, after) in before.positions.iter().zip(&after.positions) {
            assert!((after[1] - before[1] - 12.0).abs() < 1e-5);
            assert!((after[0] - before[0]).abs() < 1e-5);
            assert!((after[2] - before[2]).abs() < 1e-5);
        }
    }

    let too_low = make(serde_json::json!({
        "radius_mm":9,"height_mm":18,"offset_mm":-162.1
    }));
    assert!(pentool::scene3d::form::generate(&too_low)
        .unwrap_err()
        .to_string()
        .contains("offset_mm must keep the cap"));
    let too_high = make(serde_json::json!({
        "radius_mm":9,"height_mm":18,"offset_mm":99_821
    }));
    assert!(pentool::scene3d::form::generate(&too_high)
        .unwrap_err()
        .to_string()
        .contains("offset_mm must keep the cap"));
}

#[test]
fn dieline_spot_export_writes_independent_role_plates() {
    let document = fixture();
    let (separated, report) = pentool::scene3d::pack::dieline_spot_separations(
        &document,
        std::path::Path::new("packaging.pen"),
        "shirt-carton",
        36.0,
    )
    .unwrap();
    let spots: Vec<_> = separated.plates.iter().filter(|plate| plate.spot).collect();
    assert!(spots.iter().any(|plate| plate.id == "CutContour"));
    assert!(spots.iter().any(|plate| plate.id == "Crease"));
    assert!(spots
        .iter()
        .all(|plate| plate.samples.iter().any(|sample| *sample > 0.0)));
    assert_eq!(report["process_plates"], "blank");
    assert_eq!(report["manufacturing_certified"], false);

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("pentool-v0160-spot-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let output = dir.join("plates");
    let written = pentool::scene3d::pack::write_dieline_spot_separations(
        &document,
        std::path::Path::new("packaging.pen"),
        "shirt-carton",
        &output,
        300.0,
        8,
        "revision-test",
    )
    .unwrap();
    assert_eq!(written["document_revision"], "revision-test");
    assert_eq!(written["requested_ppi"], 300.0);
    assert!(written["pixels"]["width"].as_u64().unwrap() > 4000);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(output.join("separations.json")).unwrap()).unwrap();
    assert!(manifest["files"].as_array().unwrap().len() >= 2);
    assert!(manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["plate"] == "CutContour"
            && entry["file"]
                .as_str()
                .is_some_and(|name| name.ends_with("-spot.png"))));
    let cut_plate = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["plate"] == "CutContour")
        .unwrap();
    let cut_image = image::load_from_memory(
        &std::fs::read(output.join(cut_plate["file"].as_str().unwrap())).unwrap(),
    )
    .unwrap()
    .to_luma8();
    assert_eq!(
        cut_image.dimensions(),
        (
            written["pixels"]["width"].as_u64().unwrap() as u32,
            written["pixels"]["height"].as_u64().unwrap() as u32
        )
    );
    assert!(cut_image.pixels().any(|pixel| pixel[0] < 255));
    assert!(cut_image.pixels().any(|pixel| pixel[0] == 255));
    assert!(pentool::scene3d::pack::write_dieline_spot_separations(
        &document,
        std::path::Path::new("packaging.pen"),
        "shirt-carton",
        &output,
        36.0,
        8,
        "revision-test",
    )
    .is_err());
    let vector_output = dir.join("vector-plates");
    let vector_report = pentool::scene3d::pack::write_dieline_vector_spot_plates(
        &document,
        "shirt-carton",
        &vector_output,
    )
    .unwrap();
    assert_eq!(vector_report["units"], "mm");
    let vector_files = vector_report["plates"].as_array().unwrap();
    assert!(vector_files.iter().any(|plate| {
        let svg =
            std::fs::read_to_string(vector_output.join(plate["path"].as_str().unwrap())).unwrap();
        svg.contains("data-spot-ink=\"CutContour\"") && svg.contains("data-role=\"cut\"")
    }));
    assert!(vector_files.iter().any(|plate| plate["ink"] == "Crease"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn labeled_svg_dielines_round_trip_and_ambiguous_geometry_is_rejected_atomically() {
    let dieline = pentool::scene3d::pack::sleeve_dieline(12.0, 8.0, 10.0, 0.4, 1.0).unwrap();
    let svg = pentool::scene3d::pack::flat_dieline_svg(&dieline).unwrap();
    let mut raw = fixture();
    let result = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        &svg,
        "imported-sleeve",
        "imported-preview",
        "mockups",
        Some("products"),
        [40.0, 40.0, 200.0, 180.0],
    )
    .unwrap();
    assert_eq!(result["template"], "svg-import");
    assert_eq!(result["panels"], 4);
    pentool::scene::validate(&raw).unwrap();
    assert_eq!(
        raw["dielines"]["imported-sleeve"]["creases"][0]["direction"],
        "valley"
    );
    assert_eq!(
        pentool::scene3d::pack::generate(&raw["dielines"]["imported-sleeve"])
            .unwrap()
            .primitives
            .len(),
        4
    );

    let before = raw.clone();
    let ambiguous = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><circle cx="5" cy="5" r="2"/></svg>"#;
    let error = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        ambiguous,
        "bad-import",
        "bad-preview",
        "mockups",
        Some("products"),
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("<circle>"), "{error}");
    assert_eq!(
        raw, before,
        "rejected SVG import must not mutate the document"
    );

    let transformed_panel = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><rect data-panel="front" transform="translate(2 0)" x="0" y="0" width="10" height="10"/></svg>"#;
    let error = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        transformed_panel,
        "transformed-import",
        "transformed-preview",
        "mockups",
        Some("products"),
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("SVG transforms and CSS styles"),
        "child transforms must be rejected rather than silently ignored: {error}"
    );
    assert_eq!(raw, before, "rejected transformed SVG must be atomic");

    let overlapping_panels = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><rect data-panel="front" x="0" y="0" width="12" height="10"/><rect data-panel="back" x="10" y="0" width="10" height="10"/></svg>"#;
    let error = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        overlapping_panels,
        "overlap-import",
        "overlap-preview",
        "mockups",
        Some("products"),
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("panels `front` and `back` overlap"),
        "overlapping imported regions need a panel-specific diagnostic: {error}"
    );
    assert_eq!(
        raw, before,
        "overlapping panel import must leave the document unchanged"
    );

    let self_intersecting_cut = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><rect data-panel="front" x="0" y="0" width="20" height="10"/><path data-role="cut" data-id="bow-tie" d="M 1 1 L 9 9 L 1 9 L 9 1 Z"/></svg>"#;
    let error = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        self_intersecting_cut,
        "self-intersecting-import",
        "self-intersecting-preview",
        "mockups",
        Some("products"),
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("SVG production path `bow-tie` self-intersects"),
        "self-intersecting cut contours need a path-specific diagnostic: {error}"
    );
    assert_eq!(
        raw, before,
        "self-intersecting production geometry must not mutate the document"
    );

    let crossing_subpaths = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10"><rect data-panel="front" x="0" y="0" width="20" height="10"/><path data-role="cut" data-id="crossing-segments" d="M 1 1 L 9 9 M 1 9 L 9 1"/></svg>"#;
    let error = pentool::scene3d::pack::add_svg_dieline(
        &mut raw,
        crossing_subpaths,
        "crossing-subpaths-import",
        "crossing-subpaths-preview",
        "mockups",
        Some("products"),
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("SVG production path `crossing-segments` self-intersects"),
        "crossing cut subpaths need a path-specific diagnostic: {error}"
    );
    assert_eq!(
        raw, before,
        "crossing cut subpaths must not mutate the document"
    );

    let dir = std::env::temp_dir().join(format!("pentool-v0160-svg-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("products.pen");
    let svg_path = dir.join("sleeve.svg");
    std::fs::write(&input, serde_json::to_vec_pretty(&fixture()).unwrap()).unwrap();
    std::fs::write(&svg_path, svg.as_bytes()).unwrap();
    let args = [
        "pack",
        input.to_str().unwrap(),
        "--dry-run",
        "import",
        "cli-import",
        "--svg",
        svg_path.to_str().unwrap(),
        "--node",
        "cli-preview",
        "--layer",
        "mockups",
        "--page",
        "products",
    ];
    let source_before = std::fs::read(&input).unwrap();
    let dry = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), source_before);
    let committed = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            input.to_str().unwrap(),
            "import",
            "cli-import",
            "--svg",
            svg_path.to_str().unwrap(),
            "--node",
            "cli-preview",
            "--layer",
            "mockups",
            "--page",
            "products",
        ])
        .output()
        .unwrap();
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(stored["dielines"]["cli-import"]["template"], "svg-import");
    pentool::scene::validate(&stored).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_pdfx_is_a_flattened_draft_using_the_documents_cmyk_output_role() {
    let dir = std::env::temp_dir().join(format!("pentool-v0160-pack-pdfx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let document = dir.join("products.pen");
    let mut raw = fixture();
    pentool::color::catalog::import(
        &mut raw,
        &document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = serde_json::json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    }});
    std::fs::write(&document, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let before = raw.clone();
    let (pdf, flat) = pentool::scene3d::pack::flat_dieline_pdfx_with_artwork(
        &raw,
        &document,
        "shirt-carton",
        300.0,
        300.0,
    )
    .unwrap();
    assert!(pdf.bytes.starts_with(b"%PDF-1.6\n"));
    pentool::color::pdfx::verify_structure(&pdf.bytes).unwrap();
    assert_eq!(pdf.report["pixels"]["ppi"], 300.0);
    assert_eq!(pdf.report["output_intent"]["profile"], "press");
    assert_eq!(pdf.report["independent_validation"]["status"], "not-run");
    assert!(flat.svg.contains("data-role=\"cut\""));
    assert!(!flat.artwork_panels.is_empty());
    assert_eq!(
        raw, before,
        "PDF/X planning must not mutate the source document"
    );
    let output = dir.join("carton.pdf");
    let cli = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            document.to_str().unwrap(),
            "pack",
            "export",
            "shirt-carton",
            "--out",
            output.to_str().unwrap(),
            "--pdfx",
            "--ppi",
            "36",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let report: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(report["format"], "pdfx-4-flattened-draft");
    assert_eq!(report["manufacturing_certified"], false);
    assert_eq!(report["technical_paths"], "flattened-into-process-cmyk");
    assert!(std::fs::read(&output).unwrap().starts_with(b"%PDF-1.6\n"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_pdfx_flattens_a_generated_owner_on_the_proof_page_without_dangling_dielines() {
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-pack-pdfx-owner-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let document = dir.join("products.pen");
    let mut raw = fixture();
    raw["layout"] = serde_json::json!({"engine":1,"spreads":[
        {"id":"s1","facing":false,"pages":["artwork"]},
        {"id":"s2","facing":false,"pages":["products"]}
    ]});
    pentool::color::catalog::import(
        &mut raw,
        &document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"] = serde_json::json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    }});

    // Reproduce a carton created on a one-page document: its preview owner is
    // in the first proof page, which the PDF/X planner intentionally flattens
    // and replaces with one raster image.
    let pages = raw["pages"].as_array_mut().unwrap();
    let mut owner = None;
    for page in pages.iter_mut() {
        for layer in page["layers"].as_array_mut().unwrap() {
            let nodes = layer["nodes"].as_array_mut().unwrap();
            if let Some(index) = nodes.iter().position(|node| node["id"] == "carton-preview") {
                owner = Some(nodes.remove(index));
            }
        }
    }
    let owner = owner.expect("packaging fixture has a carton preview owner");
    pages[0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(owner);
    pentool::scene::validate(&raw).unwrap();
    pentool::scene3d::pack::flat_dieline_svg_with_artwork(&raw, &document, "shirt-carton", 300.0)
        .unwrap_or_else(|error| panic!("flat SVG planning failed: {error:#}"));

    let (pdf, _) = pentool::scene3d::pack::flat_dieline_pdfx_with_artwork(
        &raw,
        &document,
        "shirt-carton",
        300.0,
        36.0,
    )
    .unwrap_or_else(|error| panic!("PDF/X planning failed: {error:#}"));
    assert!(pdf.bytes.starts_with(b"%PDF-1.6\n"));
    pentool::color::pdfx::verify_structure(&pdf.bytes).unwrap();
    std::fs::write(&document, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let bundle = dir.join("bundle");
    let export_bundle = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            document.to_str().unwrap(),
            "export-bundle",
            "shirt-carton",
            "--out",
            bundle.to_str().unwrap(),
            "--ppi",
            "36",
            "--fold",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        export_bundle.status.success(),
        "{}",
        String::from_utf8_lossy(&export_bundle.stderr)
    );
    let verify_bundle = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify_bundle.status.success(),
        "{}",
        String::from_utf8_lossy(&verify_bundle.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_bundle_contains_revision_consistent_print_and_preview_outputs() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-pack-bundle-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("products.pen");
    let mut document = fixture();
    pentool::color::catalog::import(
        &mut document,
        &input,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    document["color_management"]["roles"] = serde_json::json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    }});
    std::fs::write(&input, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let vector_dry_out = dir.join("carton-vector-dry-run");
    let vector_dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--dry-run",
            "pack",
            "vector-separate",
            "shirt-carton",
            "--out",
            vector_dry_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        vector_dry.status.success(),
        "{}",
        String::from_utf8_lossy(&vector_dry.stderr)
    );
    let vector_dry_report: Value = serde_json::from_slice(&vector_dry.stdout).unwrap();
    assert_eq!(vector_dry_report["dry_run"], true);
    assert_eq!(vector_dry_report["complete"], false);
    assert!(!vector_dry_out.exists());
    let vector_out = dir.join("carton-vector-plates");
    let vector = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "vector-separate",
            "shirt-carton",
            "--out",
            vector_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        vector.status.success(),
        "{}",
        String::from_utf8_lossy(&vector.stderr)
    );
    assert!(vector_out.is_dir());
    let out = dir.join("carton-delivery");
    let dry_out = dir.join("carton-delivery-dry-run");
    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--dry-run",
            "pack",
            "export-bundle",
            "shirt-carton",
            "--out",
            dry_out.to_str().unwrap(),
            "--min-ppi",
            "36",
            "--ppi",
            "36",
            "--fold",
            "0.5",
        ])
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_report: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry_report["dry_run"], true);
    assert_eq!(dry_report["complete"], false);
    assert!(!dry_out.exists());
    assert_eq!(std::fs::read(&input).unwrap(), before);

    let cli = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "export-bundle",
            "shirt-carton",
            "--out",
            out.to_str().unwrap(),
            "--min-ppi",
            "36",
            "--ppi",
            "36",
            "--fold",
            "0.5",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let report: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(report["format"], "pentool-packaging-bundle-v1");
    assert_eq!(report["manufacturing_certified"], false);
    let manifest_path = out.join("manifest.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["format"], "pentool-packaging-bundle-v1");
    assert_eq!(manifest["fold_preview"], 0.5);
    assert_eq!(
        manifest["files"]["spot_separations"]["report"]["out"],
        "separations"
    );
    assert!(
        String::from_utf8(std::fs::read(out.join("flat.svg")).unwrap())
            .unwrap()
            .contains("<svg")
    );
    assert!(std::fs::read(out.join("print-draft.pdf"))
        .unwrap()
        .starts_with(b"%PDF-1.6\n"));
    assert!(std::fs::read(out.join("folded-preview.png"))
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(out.join("separations").is_dir());
    assert!(out.join("separations/separations.json").is_file());
    assert_eq!(
        manifest["files"]["vector_spot_plates"]["report"]["format"],
        "svg-vector-spot-plates"
    );
    let vector_dir = out.join("vector-spot-plates");
    assert!(vector_dir.is_dir());
    let vector_manifest = &manifest["files"]["vector_spot_plates"]["report"]["plates"];
    assert!(vector_manifest.as_array().unwrap().iter().any(|plate| {
        let data =
            std::fs::read_to_string(vector_dir.join(plate["path"].as_str().unwrap())).unwrap();
        data.contains("data-spot-ink=\"CutContour\"") && data.contains("data-role=\"cut\"")
    }));
    let verified = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let verification: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verification["status"], "verified");
    assert!(verification["artifacts_verified"].as_u64().unwrap() >= 5);
    let flat_path = out.join("flat.svg");
    let flat_bytes = std::fs::read(&flat_path).unwrap();
    std::fs::write(&flat_path, b"tampered").unwrap();
    let tampered = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!tampered.status.success());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("[integrity-error]"));
    std::fs::write(&flat_path, flat_bytes).unwrap();
    let manifest_bytes = std::fs::read(&manifest_path).unwrap();
    let mut hostile_manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    hostile_manifest["files"]["flat_svg"]["path"] = serde_json::json!("../outside.svg");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&hostile_manifest).unwrap(),
    )
    .unwrap();
    let traversal = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!traversal.status.success());
    assert!(String::from_utf8_lossy(&traversal.stderr).contains("unsafe artifact path"));
    assert_eq!(std::fs::read(&input).unwrap(), before);

    let existing = dir.join("existing-bundle");
    std::fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("preserve.txt");
    std::fs::write(&sentinel, b"keep").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "export-bundle",
            "shirt-carton",
            "--out",
            existing.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn v11_product_fixture_validates_and_plans_sources() {
    let document = fixture();
    pentool::scene::validate(&document).unwrap();
    let order = pentool::scene3d::bindings::plan(&document).unwrap();
    assert_eq!(
        order,
        ["products/carton-preview", "products/perfume-preview"]
    );

    let mut linked = document.clone();
    linked["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["source"] =
        serde_json::json!({"kind":"node", "page":"products", "node":"carton-preview"});
    let report = pentool::scene3d::bindings::plan_report(&linked).unwrap();
    assert_eq!(report["format"], "pentool-scene3d-dependency-plan-v1");
    assert_eq!(report["summary"]["nodes"], 2);
    assert_eq!(report["summary"]["edges"], 1);
    assert_eq!(
        report["render_order"][0],
        serde_json::json!({"page":"products","node":"carton-preview"})
    );
    assert_eq!(
        report["dependencies"][0]["bindings"][0],
        "graphic perfume-label slot `label-band` source node products/carton-preview"
    );
    assert_eq!(
        report,
        pentool::scene3d::bindings::plan_report(&linked).unwrap(),
        "the graph report must be deterministic"
    );

    let mut panel_page_source = document;
    panel_page_source["dielines"]["shirt-carton"]["panels"][0]["artwork"] =
        serde_json::json!({"kind":"page", "page":"products"});
    let panel_report = pentool::scene3d::bindings::plan_report(&panel_page_source).unwrap();
    assert_eq!(panel_report["summary"]["edges"], 1);
    assert!(panel_report["dependencies"][0]["bindings"][0]
        .as_str()
        .unwrap()
        .contains(
            "dieline `shirt-carton` panel `front` slot `front` artwork source page products"
        ));
}

#[test]
fn dependency_cli_reports_the_graph_without_mutating_the_document() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("pentool-v0160-plan-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("plan.pen");
    let mut document = fixture();
    document["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["source"] =
        serde_json::json!({"kind":"node", "page":"products", "node":"carton-preview"});
    std::fs::write(&input, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["scene3d", input.to_str().unwrap(), "dependencies"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["edges"], 1);
    assert_eq!(std::fs::read(&input).unwrap(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn render_report_cli_reports_actual_output_and_preserves_document_bytes() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-render-report-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("mockup.pen");
    let mut document = fixture();
    document["pages"][1]["production"] = serde_json::json!({"units_per_inch":144});
    std::fs::write(&input, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "render-report",
            "perfume-preview",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["node"], "perfume-preview");
    assert_eq!(report["page"], "products");
    assert_eq!(
        report["render"]["format"],
        "pentool-scene3d-render-report-v1"
    );
    assert!(report["render"]["output"]["width_px"].as_u64().unwrap() > 0);
    assert!(report["render"]["output"]["height_px"].as_u64().unwrap() > 0);
    assert_eq!(report["render"]["renderer_requested"], "reference");
    assert_eq!(report["render"]["renderer_used"], "reference");
    assert_eq!(report["render"]["cache_hit"], false);
    let node = &document["pages"][1]["layers"][0]["nodes"][0];
    let expected_ppi_x = report["render"]["output"]["width_px"].as_f64().unwrap()
        / node["width"].as_f64().unwrap()
        * 144.0;
    assert!(
        (report["render"]["effective_ppi"]["x"].as_f64().unwrap() - expected_ppi_x).abs() < 1e-9
    );
    assert_eq!(report["render"]["effective_ppi"]["units_per_inch"], 144.0);
    assert_eq!(std::fs::read(&input).unwrap(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn print_preflight_reports_scene3d_ppi_after_final_transform_and_threshold() {
    let document_path = std::path::Path::new("scene3d-preflight.pen");
    let options = pentool::color::preflight::Options {
        min_ppi: 150.0,
        bleed_scale: None,
        ..Default::default()
    };
    let mut document = fixture();
    document["pages"][1]["production"] = serde_json::json!({"units_per_inch":144});
    let placed_width = document["pages"][1]["layers"][0]["nodes"][0]["width"].clone();

    let low = pentool::color::preflight::run(&document, document_path, Some("products"), &options)
        .unwrap();
    let ppi = low["scene3d_ppi"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["node"] == "perfume-preview")
        .unwrap();
    assert_eq!(ppi["width_px"], placed_width);
    assert_eq!(ppi["effective_ppi"]["x"], 144.0);
    assert_eq!(ppi["effective_ppi"]["y"], 144.0);
    assert!(low["issues"].as_array().unwrap().iter().any(|issue| {
        issue["code"] == "scene3d-low-resolution" && issue["node"] == "perfume-preview"
    }));

    document["pages"][1]["layers"][0]["nodes"][0]["pixel_ratio"] = serde_json::json!(2.0);
    document["pages"][1]["layers"][0]["nodes"][0]["transform"] =
        serde_json::json!([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    let rotated =
        pentool::color::preflight::run(&document, document_path, Some("products"), &options)
            .unwrap();
    let ppi = rotated["scene3d_ppi"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["node"] == "perfume-preview")
        .unwrap();
    assert_eq!(ppi["width_px"], 960);
    assert_eq!(ppi["effective_ppi"]["x"], 288.0);
    assert_eq!(ppi["effective_ppi"]["y"], 288.0);
    assert!(!rotated["issues"].as_array().unwrap().iter().any(|issue| {
        issue["code"] == "scene3d-low-resolution" && issue["node"] == "perfume-preview"
    }));

    document["pages"][1]["layers"][0]["nodes"][0]["transform"] =
        serde_json::json!([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    let scaled =
        pentool::color::preflight::run(&document, document_path, Some("products"), &options)
            .unwrap();
    let ppi = scaled["scene3d_ppi"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["node"] == "perfume-preview")
        .unwrap();
    assert_eq!(ppi["effective_ppi"]["x"], 144.0);
    assert_eq!(ppi["placed_width"], 960.0);
    assert!(scaled["issues"].as_array().unwrap().iter().any(|issue| {
        issue["code"] == "scene3d-low-resolution" && issue["node"] == "perfume-preview"
    }));
}

#[test]
fn batch_render_cli_publishes_dependency_ordered_outputs_atomically() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-batch-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("products.pen");
    let out_dir = dir.join("batch-output");
    let document = fixture();
    std::fs::write(&input, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();

    let mut oversized = document.clone();
    let nodes = oversized["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap();
    for node in nodes.iter_mut() {
        node["width"] = serde_json::json!(4000);
        node["height"] = serde_json::json!(4000);
        node["supersample"] = serde_json::json!(1);
    }
    let template = nodes[0].clone();
    for index in 0..3 {
        let mut extra = template.clone();
        extra["id"] = serde_json::json!(format!("extra-mockup-{index}"));
        nodes.push(extra);
    }
    let oversized_input = dir.join("oversized.pen");
    std::fs::write(
        &oversized_input,
        serde_json::to_vec_pretty(&oversized).unwrap(),
    )
    .unwrap();
    let oversized_out = dir.join("oversized-output");
    let mut oversized_args = vec![
        "scene3d".to_owned(),
        oversized_input.to_string_lossy().into_owned(),
        "batch".to_owned(),
    ];
    for id in [
        "perfume-preview",
        "carton-preview",
        "extra-mockup-0",
        "extra-mockup-1",
        "extra-mockup-2",
    ] {
        oversized_args.push("--node".into());
        oversized_args.push(id.into());
    }
    oversized_args.push("--out-dir".into());
    oversized_args.push(oversized_out.to_string_lossy().into_owned());
    let over_limit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(oversized_args)
        .output()
        .unwrap();
    assert!(!over_limit.status.success());
    assert!(String::from_utf8_lossy(&over_limit.stderr).contains("64000000 total pixels"));
    assert!(
        !oversized_out.exists(),
        "over-limit batch must not render/publish"
    );

    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--dry-run",
            "batch",
            "--node",
            "perfume-preview",
            "--node",
            "carton-preview",
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_report: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry_report["summary"]["dry_run"], true);
    assert_eq!(dry_report["summary"]["complete"], false);
    assert!(!out_dir.exists(), "dry-run must not create output files");
    assert_eq!(std::fs::read(&input).unwrap(), before);

    let output = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "batch",
            "--node",
            "perfume-preview",
            "--node",
            "carton-preview",
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["format"], "pentool-scene3d-batch-report-v1");
    assert_eq!(report["summary"]["rendered"], 2);
    assert_eq!(report["summary"]["complete"], true);
    let items = report["items"].as_array().unwrap();
    assert_eq!(items[0]["node"], "carton-preview");
    assert_eq!(items[1]["node"], "perfume-preview");
    for item in items {
        let path = std::path::PathBuf::from(item["file"].as_str().unwrap());
        let bytes = std::fs::read(path).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(item["bytes"].as_u64().unwrap(), bytes.len() as u64);
        assert_eq!(
            item["sha256"],
            pentool::resource::sha256(&bytes),
            "batch item digest must match its delivered file"
        );
    }
    assert_eq!(std::fs::read(&input).unwrap(), before);

    let repeated_dir = dir.join("batch-output-repeat");
    let repeated = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "batch",
            "--node",
            "perfume-preview",
            "--node",
            "carton-preview",
            "--out-dir",
            repeated_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_report: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    for (first, second) in items
        .iter()
        .zip(repeated_report["items"].as_array().unwrap())
    {
        assert_eq!(first["node"], second["node"]);
        assert_eq!(first["sha256"], second["sha256"]);
        assert_eq!(
            std::path::Path::new(first["file"].as_str().unwrap()).file_name(),
            std::path::Path::new(second["file"].as_str().unwrap()).file_name()
        );
    }

    let existing = dir.join("existing-output");
    std::fs::create_dir(&existing).unwrap();
    let sentinel = existing.join("keep.txt");
    std::fs::write(&sentinel, b"preserve").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "batch",
            "--node",
            "perfume-preview",
            "--out-dir",
            existing.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(std::fs::read(sentinel).unwrap(), b"preserve");

    // Node IDs are page-local: batch selectors must be able to address two
    // same-named mockups without relying on accidental global uniqueness.
    let mut multi_page = fixture();
    let mut same_id = multi_page["pages"][1]["layers"][0]["nodes"][0].clone();
    same_id["width"] = serde_json::json!(96);
    same_id["height"] = serde_json::json!(96);
    same_id["surface_graphics"] = serde_json::json!([]);
    multi_page["pages"][1]["layers"][0]["nodes"][0] = same_id.clone();
    let mut second_page = multi_page["pages"][1].clone();
    second_page["id"] = serde_json::json!("products-alt");
    second_page["layers"][0]["nodes"] = serde_json::json!([same_id]);
    multi_page["pages"]
        .as_array_mut()
        .unwrap()
        .push(second_page);
    let multi_input = dir.join("multi-page.pen");
    std::fs::write(&multi_input, serde_json::to_vec(&multi_page).unwrap()).unwrap();
    let ambiguous_dir = dir.join("ambiguous-output");
    let ambiguous = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            multi_input.to_str().unwrap(),
            "batch",
            "--node",
            "perfume-preview",
            "--out-dir",
            ambiguous_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("use PAGE::perfume-preview"));
    assert!(!ambiguous_dir.exists());
    let qualified_dir = dir.join("qualified-output");
    let qualified = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            multi_input.to_str().unwrap(),
            "batch",
            "--node",
            "products::perfume-preview",
            "--node",
            "products-alt::perfume-preview",
            "--out-dir",
            qualified_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        qualified.status.success(),
        "{}",
        String::from_utf8_lossy(&qualified.stderr)
    );
    let qualified_report: Value = serde_json::from_slice(&qualified.stdout).unwrap();
    let qualified_items = qualified_report["items"].as_array().unwrap();
    assert_eq!(qualified_items.len(), 2);
    assert_eq!(qualified_items[0]["page"], "products");
    assert_eq!(qualified_items[1]["page"], "products-alt");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_queue_preflights_multiple_documents_and_cancels_atomically() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-batch-queue-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut first_doc = fixture();
    for node in first_doc["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
    {
        node["width"] = serde_json::json!(72);
        node["height"] = serde_json::json!(72);
        node["supersample"] = serde_json::json!(1);
    }
    first_doc["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"] = serde_json::json!([]);
    let mut second_doc = first_doc.clone();
    second_doc["pages"][1]["layers"][0]["nodes"][0]["camera"]["azimuth"] = serde_json::json!(60);
    let first_path = dir.join("first.pen");
    let second_path = dir.join("second.pen");
    std::fs::write(&first_path, serde_json::to_vec(&first_doc).unwrap()).unwrap();
    std::fs::write(&second_path, serde_json::to_vec(&second_doc).unwrap()).unwrap();
    let originals = [
        std::fs::read(&first_path).unwrap(),
        std::fs::read(&second_path).unwrap(),
    ];
    let manifest_path = dir.join("queue.json");
    let manifest = serde_json::json!({
        "format":"pentool-scene3d-batch-queue-v1",
        "jobs":[
            {"document":"first.pen","nodes":["carton-preview","perfume-preview"]},
            {"document":"second.pen","nodes":["carton-preview","perfume-preview"]}
        ]
    });
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let output_dir = dir.join("queue-output");

    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "--dry-run",
            "batch-queue",
            "--out-dir",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let dry_report: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry_report["summary"]["jobs"], 2);
    assert_eq!(dry_report["summary"]["rendered"], 4);
    assert_eq!(dry_report["summary"]["complete"], false);
    assert!(!output_dir.exists());

    let cancel_file = dir.join("cancel.request");
    std::fs::write(&cancel_file, b"cancel").unwrap();
    let cancelled_dir = dir.join("cancelled-output");
    let cancelled = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            cancelled_dir.to_str().unwrap(),
            "--cancel-file",
            cancel_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!cancelled.status.success());
    assert!(String::from_utf8_lossy(&cancelled.stderr).contains("[cancelled]"));
    assert!(!cancelled_dir.exists());
    std::fs::remove_file(cancel_file).unwrap();

    let rendered = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let report: Value = serde_json::from_slice(&rendered.stdout).unwrap();
    assert_eq!(report["summary"]["rendered"], 4);
    assert_eq!(report["summary"]["complete"], true);
    for job in report["jobs"].as_array().unwrap() {
        for item in job["items"].as_array().unwrap() {
            let file = std::path::PathBuf::from(item["file"].as_str().unwrap());
            let bytes = std::fs::read(file).unwrap();
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert_eq!(item["sha256"], pentool::resource::sha256(&bytes));
        }
    }
    assert_eq!(std::fs::read(&first_path).unwrap(), originals[0]);
    assert_eq!(std::fs::read(&second_path).unwrap(), originals[1]);

    let mut oversized = first_doc.clone();
    for node in oversized["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
    {
        node["width"] = serde_json::json!(4000);
        node["height"] = serde_json::json!(4000);
    }
    let oversized_path = dir.join("oversized.pen");
    std::fs::write(&oversized_path, serde_json::to_vec(&oversized).unwrap()).unwrap();
    let over_manifest = serde_json::json!({
        "format":"pentool-scene3d-batch-queue-v1",
        "jobs":[
            {"document":"oversized.pen","nodes":["carton-preview","perfume-preview"]},
            {"document":"oversized.pen","nodes":["carton-preview","perfume-preview"]},
            {"document":"oversized.pen","nodes":["carton-preview","perfume-preview"]}
        ]
    });
    std::fs::write(&manifest_path, serde_json::to_vec(&over_manifest).unwrap()).unwrap();
    let over_dir = dir.join("over-limit-output");
    let over = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            over_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!over.status.success());
    let over_error = String::from_utf8_lossy(&over.stderr);
    assert!(
        over_error.contains("batch queue estimated PNG data exceeds")
            || over_error.contains("batch queue exceeds 64000000 total pixels"),
        "aggregate preflight should reject before rendering: {over_error}"
    );
    assert!(
        !over_dir.exists(),
        "aggregate preflight must run before publication"
    );

    let mut supersampled = first_doc.clone();
    for node in supersampled["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
    {
        node["width"] = serde_json::json!(2048);
        node["height"] = serde_json::json!(2048);
        node["supersample"] = serde_json::json!(4);
    }
    let supersampled_path = dir.join("supersampled.pen");
    std::fs::write(
        &supersampled_path,
        serde_json::to_vec(&supersampled).unwrap(),
    )
    .unwrap();
    let supersampled_manifest = serde_json::json!({
        "format":"pentool-scene3d-batch-queue-v1",
        "jobs":[
            {"document":"supersampled.pen","nodes":["carton-preview","perfume-preview"]},
            {"document":"supersampled.pen","nodes":["carton-preview","perfume-preview"]},
            {"document":"supersampled.pen","nodes":["carton-preview","perfume-preview"]}
        ]
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&supersampled_manifest).unwrap(),
    )
    .unwrap();
    let supersampled_dir = dir.join("supersampled-output");
    let over_samples = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            supersampled_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!over_samples.status.success());
    assert!(String::from_utf8_lossy(&over_samples.stderr)
        .contains("batch queue exceeds 268435456 supersampled samples"));
    assert!(!supersampled_dir.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_queue_isolates_item_render_failures_and_publishes_successes() {
    use std::process::Command;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-batch-isolation-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut document = fixture();
    document["pages"][1]["layers"][0]["nodes"][0]["width"] = serde_json::json!(96);
    document["pages"][1]["layers"][0]["nodes"][0]["height"] = serde_json::json!(96);
    document["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"] = serde_json::json!([]);
    let broken_digest = format!("sha256:{}", "0".repeat(64));
    document["models"][&broken_digest] = serde_json::json!({
        "format":"obj",
        "bytes":59,
        "storage":{"kind":"external","path":"missing-model.obj"}
    });
    let broken_node = &mut document["pages"][1]["layers"][0]["nodes"][1];
    broken_node.as_object_mut().unwrap().remove("dieline");
    broken_node["model"] = serde_json::json!(broken_digest);
    document["dielines"]
        .as_object_mut()
        .unwrap()
        .remove("shirt-carton");
    let document_path = dir.join("mixed.pen");
    std::fs::write(&document_path, serde_json::to_vec(&document).unwrap()).unwrap();
    let manifest_path = dir.join("queue.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "format":"pentool-scene3d-batch-queue-v1",
            "jobs":[{"document":"mixed.pen","nodes":["perfume-preview","carton-preview"]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let output_dir = dir.join("partial-output");
    let output = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["requested"], 2);
    assert_eq!(report["summary"]["rendered"], 1, "{report}");
    assert_eq!(report["summary"]["failed"], 1);
    assert_eq!(report["summary"]["complete"], false);
    assert_eq!(report["summary"]["partial"], true);
    let items = report["jobs"][0]["items"].as_array().unwrap();
    assert_eq!(items[0]["status"], "rendered");
    let png = std::fs::read(items[0]["file"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(items[1]["status"], "failed");
    assert!(items[1]["error"]
        .as_str()
        .unwrap()
        .contains("missing-model.obj"));
    assert!(!items[1].get("file").is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_queue_cancellation_after_a_render_discards_staged_successes() {
    use std::{
        ffi::OsStr,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    fn contains_png(directory: &std::path::Path) -> bool {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            path.extension() == Some(OsStr::new("png")) || (path.is_dir() && contains_png(&path))
        })
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pentool-v0160-batch-mid-cancel-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let mut document = fixture();
    for node in document["pages"][1]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
    {
        node["width"] = serde_json::json!(1400);
        node["height"] = serde_json::json!(1400);
        node["pixel_ratio"] = serde_json::json!(1);
        node["supersample"] = serde_json::json!(1);
        node["surface_graphics"] = serde_json::json!([]);
    }
    let input = dir.join("products.pen");
    std::fs::write(&input, serde_json::to_vec(&document).unwrap()).unwrap();
    let manifest_path = dir.join("queue.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "format":"pentool-scene3d-batch-queue-v1",
            "jobs":[{"document":"products.pen","nodes":["carton-preview","perfume-preview"]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let marker = dir.join("cancel-after-first.png");
    let output = dir.join("queue-output");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            manifest_path.to_str().unwrap(),
            "batch-queue",
            "--out-dir",
            output.to_str().unwrap(),
            "--cancel-file",
            marker.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut cancelled_after_first = false;
    while Instant::now() < deadline {
        if contains_png(&dir) {
            std::fs::write(&marker, b"cancel").unwrap();
            cancelled_after_first = true;
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("batch queue exited before staging its first render: {status}");
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        cancelled_after_first,
        "queue never staged its first rendered PNG"
    );

    let result = child.wait_with_output().unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("[cancelled]"),
        "mid-run cancellation should be reported: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !output.exists(),
        "a mid-run cancellation must not publish the successful staged item"
    );
    assert!(
        std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".pentool-batch-queue-")),
        "cancelled queue should remove its staging tree"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn v11_missing_binding_source_fails_document_validation() {
    let mut document = fixture();
    document["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["source"]["page"] =
        Value::String("missing-page".into());
    let error = pentool::scene::validate(&document).unwrap_err().to_string();
    assert!(error.contains("[missing-resource]"), "{error}");
    for detail in [
        "source page `missing-page`",
        "consumer products/perfume-preview",
        "slot `label-band`",
        "update/clear the binding",
    ] {
        assert!(
            error.contains(detail),
            "missing actionable detail {detail}: {error}"
        );
    }
}

#[test]
fn v11_dependency_cycle_fails_document_validation() {
    let mut document = fixture();
    let nodes = &mut document["pages"][1]["layers"][0]["nodes"];
    nodes[0]["surface_graphics"][0]["source"] =
        serde_json::json!({"kind":"node", "page":"products", "node":"carton-preview"});
    nodes[1]["surface_graphics"] = serde_json::json!([{
        "id":"carton-art", "source":{"kind":"node", "page":"products", "node":"perfume-preview"},
        "slot":"front", "mode":"decal", "projection":"uv"
    }]);
    let error = pentool::scene::validate(&document).unwrap_err().to_string();
    assert!(error.contains("[dependency-cycle]"), "{error}");
    for detail in [
        "consumer products/perfume-preview",
        "source products/carton-preview",
        "graphic perfume-label",
        "slot `label-band`",
        "remove or rebind",
    ] {
        assert!(
            error.contains(detail),
            "missing actionable detail {detail}: {error}"
        );
    }
    let consumer = document["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let render_error = pentool::scene3d::node::render_png_in_page(
        &document,
        std::path::Path::new("."),
        Some("products"),
        consumer,
    )
    .unwrap_err()
    .to_string();
    assert!(
        render_error.contains("[dependency-cycle]"),
        "direct render must enforce the shared binding plan: {render_error}"
    );
}

#[test]
fn v11_panel_art_dependency_cycle_reports_panel_and_slot() {
    let mut document = fixture();
    document["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["source"] =
        serde_json::json!({"kind":"node", "page":"products", "node":"z-carton-preview"});
    document["pages"][1]["layers"][0]["nodes"][1]["id"] = serde_json::json!("z-carton-preview");
    document["dielines"]["shirt-carton"]["scene3d_node"] = serde_json::json!("z-carton-preview");
    document["dielines"]["shirt-carton"]["panels"][0]["artwork"] = serde_json::json!({
        "kind":"node", "page":"products", "node":"perfume-preview"
    });
    let error = pentool::scene::validate(&document).unwrap_err().to_string();
    assert!(error.contains("[dependency-cycle]"), "{error}");
    for detail in [
        "consumer products/z-carton-preview",
        "source products/perfume-preview",
        "dieline `shirt-carton` panel `front` slot `front`",
        "remove or rebind",
    ] {
        assert!(
            error.contains(detail),
            "missing actionable detail {detail}: {error}"
        );
    }
}

#[test]
fn v11_dielines_reject_bad_panels_dimensions_sources_and_owner_references() {
    let mut duplicate_panel = fixture();
    duplicate_panel["dielines"]["shirt-carton"]["panels"][1]["id"] = serde_json::json!("front");

    let mut overlapping_panels = fixture();
    overlapping_panels["dielines"]["shirt-carton"]["panels"][1]["rect_mm"][0] =
        serde_json::json!(100);

    let mut bad_board = fixture();
    bad_board["dielines"]["shirt-carton"]["dimensions_mm"]["board"] = serde_json::json!(200);

    let mut missing_artwork = fixture();
    missing_artwork["dielines"]["shirt-carton"]["panels"][0]["artwork"]["page"] =
        serde_json::json!("not-a-page");

    let mut dangling_node_reference = fixture();
    dangling_node_reference["pages"][1]["layers"][0]["nodes"][1]["dieline"] =
        serde_json::json!("missing-dieline");

    for document in [
        duplicate_panel,
        overlapping_panels,
        bad_board,
        missing_artwork,
        dangling_node_reference,
    ] {
        assert!(pentool::scene::validate(&document).is_err());
    }
}

#[test]
fn generated_perfume_form_renders_on_the_cpu_path() {
    let mut document = fixture();
    document["product_forms"]["perfume-bottle"]["parameters"]["cap_mm"] =
        serde_json::json!({"radius_mm":9,"height_mm":18});
    document["product_forms"]["perfume-bottle"]["material_slots"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!("cap"));
    {
        let node = &mut document["pages"][1]["layers"][0]["nodes"][0];
        node["width"] = serde_json::json!(80);
        node["height"] = serde_json::json!(110);
        node["supersample"] = serde_json::json!(1);
    }
    pentool::scene::validate(&document).unwrap();
    let node = document["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let png = pentool::scene3d::node::render_png_in_page(
        &document,
        std::path::Path::new("."),
        Some("products"),
        node,
    )
    .unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let mut unrelated_page_edit = document.clone();
    let mut unrelated_page = unrelated_page_edit["pages"][0].clone();
    unrelated_page["id"] = serde_json::json!("unrelated-page");
    unrelated_page_edit["pages"]
        .as_array_mut()
        .unwrap()
        .push(unrelated_page);
    let unchanged_node = unrelated_page_edit["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let unchanged_png = pentool::scene3d::node::render_png_in_page(
        &unrelated_page_edit,
        std::path::Path::new("."),
        Some("products"),
        unchanged_node,
    )
    .unwrap();
    assert_eq!(
        png, unchanged_png,
        "unrelated page additions must not change a linked mockup"
    );
    let mut unrelated_node_edit = document.clone();
    unrelated_node_edit["pages"][1]["layers"][0]["nodes"][1]["x"] = serde_json::json!(617);
    let unrelated_edit_node = unrelated_node_edit["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let unaffected_png = pentool::scene3d::node::render_png_in_page(
        &unrelated_node_edit,
        std::path::Path::new("."),
        Some("products"),
        unrelated_edit_node,
    )
    .unwrap();
    assert_eq!(
        png, unaffected_png,
        "editing a sibling mockup must not change the selected mockup pixels"
    );
    let mut edited_source = document.clone();
    edited_source["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        serde_json::json!("#d02040");
    let edited_node = edited_source["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let edited_png = pentool::scene3d::node::render_png_in_page(
        &edited_source,
        std::path::Path::new("."),
        Some("products"),
        edited_node,
    )
    .unwrap();
    assert_ne!(
        png, edited_png,
        "edited source art must invalidate the mockup cache"
    );
    for fit in ["cover", "stretch"] {
        let mut fitted = document.clone();
        fitted["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["fit"] =
            serde_json::json!(fit);
        let fitted_node = fitted["pages"][1]["layers"][0]["nodes"][0]
            .as_object()
            .unwrap();
        let fitted_png = pentool::scene3d::node::render_png_in_page(
            &fitted,
            std::path::Path::new("."),
            Some("products"),
            fitted_node,
        )
        .unwrap();
        assert_ne!(
            png, fitted_png,
            "{fit} fit must affect the linked mockup pixels"
        );
    }
    let mut cropped = document.clone();
    cropped["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["crop"] =
        serde_json::json!([0.1, 0.1, 0.7, 0.7]);
    let cropped_node = cropped["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let cropped_png = pentool::scene3d::node::render_png_in_page(
        &cropped,
        std::path::Path::new("."),
        Some("products"),
        cropped_node,
    )
    .unwrap();
    assert_ne!(png, cropped_png, "crop bounds must affect linked artwork");
    let mut planar = document.clone();
    planar["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["projection"] =
        serde_json::json!("planar");
    planar["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["axis"] =
        serde_json::json!("z");
    let planar_node = planar["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let planar_png = pentool::scene3d::node::render_png_in_page(
        &planar,
        std::path::Path::new("."),
        Some("products"),
        planar_node,
    )
    .unwrap();
    assert_ne!(
        png, planar_png,
        "planar projection must use the selected mesh axis"
    );
    for projection in ["box", "cylindrical", "spherical"] {
        let mut projected = document.clone();
        projected["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["projection"] =
            serde_json::json!(projection);
        projected["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["axis"] =
            serde_json::json!("y");
        pentool::scene::validate(&projected).unwrap();
        let projected_node = projected["pages"][1]["layers"][0]["nodes"][0]
            .as_object()
            .unwrap();
        let projected_png = pentool::scene3d::node::render_png_in_page(
            &projected,
            std::path::Path::new("."),
            Some("products"),
            projected_node,
        )
        .unwrap();
        assert_ne!(
            png, projected_png,
            "{projection} projection must affect the wrapped artwork"
        );
    }
    let mut triplanar = document.clone();
    triplanar["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["projection"] =
        serde_json::json!("triplanar");
    let triplanar_node = triplanar["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    pentool::scene::validate(&triplanar).unwrap();
    let triplanar_png = pentool::scene3d::node::render_png_in_page(
        &triplanar,
        std::path::Path::new("."),
        Some("products"),
        triplanar_node,
    )
    .unwrap();
    assert_ne!(
        png, triplanar_png,
        "triplanar projection must affect wrapped artwork"
    );
    let mut fill_once = document.clone();
    fill_once["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["mode"] =
        serde_json::json!("fill");
    fill_once["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["scale"] =
        serde_json::json!([1, 1]);
    let fill_one_node = fill_once["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let fill_one = pentool::scene3d::node::render_png_in_page(
        &fill_once,
        std::path::Path::new("."),
        Some("products"),
        fill_one_node,
    )
    .unwrap();
    let mut fill_repeat = fill_once.clone();
    fill_repeat["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["scale"] =
        serde_json::json!([2, 1]);
    fill_repeat["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["wrap"] =
        serde_json::json!("repeat");
    let fill_repeat_node = fill_repeat["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let fill_two = pentool::scene3d::node::render_png_in_page(
        &fill_repeat,
        std::path::Path::new("."),
        Some("products"),
        fill_repeat_node,
    )
    .unwrap();
    assert_ne!(
        fill_one, fill_two,
        "fill repeat count must change the wrapped surface"
    );
    let mut fill_mirror = fill_repeat.clone();
    fill_mirror["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["wrap"] =
        serde_json::json!("mirror");
    let mirrored_node = fill_mirror["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let mirrored = pentool::scene3d::node::render_png_in_page(
        &fill_mirror,
        std::path::Path::new("."),
        Some("products"),
        mirrored_node,
    )
    .unwrap();
    assert_ne!(
        fill_two, mirrored,
        "mirror wrap must flip alternating fill tiles"
    );
    let mut multiply = document.clone();
    multiply["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["blend"] =
        serde_json::json!("multiply");
    let multiply_node = multiply["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let multiplied = pentool::scene3d::node::render_png_in_page(
        &multiply,
        std::path::Path::new("."),
        Some("products"),
        multiply_node,
    )
    .unwrap();
    assert_ne!(
        png, multiplied,
        "graphic blend modes must affect rendered pixels"
    );
    let mut without_graphic = document.clone();
    without_graphic["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"] =
        serde_json::json!([]);
    let plain_node = without_graphic["pages"][1]["layers"][0]["nodes"][0]
        .as_object()
        .unwrap();
    let plain = pentool::scene3d::node::render_png_in_page(
        &without_graphic,
        std::path::Path::new("."),
        Some("products"),
        plain_node,
    )
    .unwrap();
    assert_ne!(
        png, plain,
        "linked page artwork must affect rendered pixels"
    );
    let carton = document["pages"][1]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let png =
        pentool::scene3d::node::render_png(&document, std::path::Path::new("."), carton).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let mut flat_document = document.clone();
    flat_document["dielines"]["shirt-carton"]["fold_state"] = serde_json::json!(0);
    let flat_carton = flat_document["pages"][1]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let flat_png =
        pentool::scene3d::node::render_png(&flat_document, std::path::Path::new("."), flat_carton)
            .unwrap();
    assert_ne!(
        png, flat_png,
        "fold state must change the rendered carton geometry"
    );
}

#[test]
fn generated_sleeve_renders_at_flat_and_folded_states() {
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(320, 240)).unwrap();
    let mut document = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    let dieline_id = "sample-sleeve";
    let node_id = "sample-sleeve-preview";
    let mut dieline = pentool::scene3d::pack::sleeve_dieline(120.0, 70.0, 180.0, 0.5, 3.0).unwrap();
    let page_id = document["pages"][0]["id"].as_str().unwrap().to_owned();
    dieline["panels"][0]["artwork"] = serde_json::json!({"kind":"page","page":page_id});
    dieline["scene3d_node"] = serde_json::json!(node_id);
    document["dielines"][dieline_id] = dieline;
    document["pages"][0]["layers"][0]["nodes"] = serde_json::json!([
        {"kind":"rect","id":"sleeve-art","x":0,"y":0,"width":120,"height":180,"style":{"fill":{"fallback":"#d02040"}}},
        {"kind":"scene3d", "id":node_id, "x":20, "y":20, "width":220, "height":180,"dieline":dieline_id}
    ]);
    pentool::scene::validate(&document).unwrap();
    let folded_node = document["pages"][0]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let folded =
        pentool::scene3d::node::render_png(&document, std::path::Path::new("."), folded_node)
            .unwrap();
    assert!(folded.starts_with(b"\x89PNG\r\n\x1a\n"));

    let mut unlinked_document = document.clone();
    unlinked_document["dielines"][dieline_id]["panels"][0]
        .as_object_mut()
        .unwrap()
        .remove("artwork");
    let unlinked_node = unlinked_document["pages"][0]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let unlinked = pentool::scene3d::node::render_png(
        &unlinked_document,
        std::path::Path::new("."),
        unlinked_node,
    )
    .unwrap();
    assert_ne!(
        folded, unlinked,
        "linked panel art must appear on its folded face"
    );

    let mut flat_document = document.clone();
    flat_document["dielines"][dieline_id]["fold_state"] = serde_json::json!(0.0);
    let flat_node = flat_document["pages"][0]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let flat =
        pentool::scene3d::node::render_png(&flat_document, std::path::Path::new("."), flat_node)
            .unwrap();
    assert_ne!(
        folded, flat,
        "generated sleeve fold state must change the CPU render"
    );
}

#[test]
fn generated_tuck_carton_links_panel_art_and_renders_flat_and_folded() {
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(320, 240)).unwrap();
    let mut document = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    let page_id = document["pages"][0]["id"].as_str().unwrap().to_owned();
    let layer_id = document["pages"][0]["layers"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    document["pages"][0]["layers"][0]["nodes"] = serde_json::json!([{
        "kind":"rect","id":"carton-art","x":0,"y":0,"width":140,"height":180,
        "style":{"fill":{"fallback":"#d02040"}}
    }]);
    pentool::scene3d::pack::add_tuck_carton(
        &mut document,
        "sample-carton",
        "sample-carton-preview",
        &layer_id,
        Some(&page_id),
        "straight-tuck",
        120.0,
        70.0,
        180.0,
        0.5,
        3.0,
        15.0,
        [20.0, 20.0, 240.0, 200.0],
    )
    .unwrap();
    let preflight = pentool::scene3d::pack::preflight(
        &document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    let cut_check = preflight["production_path_checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|path| path["id"] == "carton-cut-outline")
        .unwrap();
    assert_eq!(cut_check["self_intersects"], false, "{preflight}");
    let layout_page_id = document["dielines"]["sample-carton"]["layout_page"]
        .as_str()
        .unwrap();
    let layout_page = document["pages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|page| page["id"] == layout_page_id)
        .unwrap();
    assert_eq!(layout_page["dieline_layout"]["dieline"], "sample-carton");
    let editable_kinds: std::collections::BTreeSet<&str> = layout_page["layers"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|layer| layer["nodes"].as_array().unwrap())
        .filter_map(|node| node["kind"].as_str())
        .collect();
    assert!(editable_kinds.contains("rect"));
    assert!(editable_kinds.contains("text"));
    assert!(editable_kinds.contains("path"));
    pentool::scene3d::bindings::set_panel_artwork(
        &mut document,
        "sample-carton",
        "front",
        Some(&serde_json::json!({"kind":"page","page":page_id})),
    )
    .unwrap();
    pentool::scene3d::bindings::set_panel_artwork(
        &mut document,
        "sample-carton",
        "back",
        Some(&serde_json::json!({"kind":"node","page":page_id,"node":"carton-art"})),
    )
    .unwrap();
    let flat_default = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    let preview_node = &document["pages"][0]["layers"][0]["nodes"][1];
    let preview_default = pentool::scene3d::node::render_png(
        &document,
        std::path::Path::new("."),
        preview_node.as_object().unwrap(),
    )
    .unwrap();
    let mut invalid_transform = document.clone();
    assert!(pentool::scene3d::bindings::set_panel_artwork_transform(
        &mut invalid_transform,
        "sample-carton",
        "front",
        [0.5, 0.5],
        [0.1, 0.5],
        0.0,
    )
    .is_err());
    assert_eq!(invalid_transform, document);
    let transform_report = pentool::scene3d::bindings::set_panel_artwork_transform(
        &mut document,
        "sample-carton",
        "front",
        [0.5, 0.5],
        [0.6, 0.45],
        17.0,
    )
    .unwrap();
    assert_eq!(transform_report["artwork_transform"]["rotate"], 17.0);
    pentool::scene::validate(&document).unwrap();
    let preview_node = &document["pages"][0]["layers"][0]["nodes"][1];
    let preview_transformed = pentool::scene3d::node::render_png(
        &document,
        std::path::Path::new("."),
        preview_node.as_object().unwrap(),
    )
    .unwrap();
    assert_ne!(preview_default, preview_transformed);
    let flat_export = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    assert_eq!(flat_export.artwork_panels, ["front", "back"]);
    assert!(flat_export.rasterized_panels.is_empty());
    assert_eq!(flat_export.bleed_synthesized.len(), 2);
    assert!(flat_export
        .bleed_synthesized
        .iter()
        .all(|panel| panel["bleed_mm"] == 3.0));
    assert!(flat_export.svg.contains("data-panel-bleed=\"front\""));
    assert!(flat_export.svg.contains("data-panel-bleed=\"back\""));
    assert!(flat_export.svg.contains("fill=\"#d02040\""));
    assert!(flat_export.svg.contains("data-artwork-panel=\"front\""));
    assert!(flat_export.svg.contains("data-artwork-panel=\"back\""));
    assert_ne!(flat_export.svg, flat_default.svg);
    let front_artwork = flat_export
        .svg
        .find("data-artwork-panel=\"front\"")
        .map(|start| &flat_export.svg[start..(start + 500).min(flat_export.svg.len())])
        .unwrap_or("front artwork element missing");
    let placement_attrs = front_artwork
        .split_once("<svg ")
        .and_then(|(_, tail)| tail.split_once('>').map(|(attrs, _)| attrs))
        .expect("front artwork SVG placement attributes");
    let coordinate = |name: &str| {
        placement_attrs
            .split_once(&format!("{name}=\""))
            .and_then(|(_, tail)| tail.split_once('\"').map(|(value, _)| value))
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    for (name, expected) in [("y", 36.0), ("width", 60.0), ("height", 90.0)] {
        assert!(
            (coordinate(name) - expected).abs() < 1.0e-8,
            "unexpected {name}: {placement_attrs}"
        );
    }
    assert_eq!(coordinate("x"), 42.0, "unexpected x: {placement_attrs}");
    let vector_preview = pentool::render::svg_to_png(&flat_export.svg, 800, 900, 1.0).unwrap();
    assert!(vector_preview.starts_with(b"\x89PNG\r\n\x1a\n"));
    let marks_only =
        pentool::scene3d::pack::flat_dieline_svg(&document["dielines"]["sample-carton"]).unwrap();
    let marks_preview = pentool::render::svg_to_png(&marks_only, 800, 900, 1.0).unwrap();
    assert_ne!(
        pentool::resource::sha256(&vector_preview),
        pentool::resource::sha256(&marks_preview),
        "linked vector artwork must render"
    );
    let mut invalid_safe_zone = document.clone();
    assert!(pentool::scene3d::bindings::set_panel_safe_zone(
        &mut invalid_safe_zone,
        "sample-carton",
        "front",
        61.0,
    )
    .is_err());
    assert_eq!(invalid_safe_zone, document);
    pentool::scene3d::bindings::set_panel_safe_zone(&mut document, "sample-carton", "front", 25.0)
        .unwrap();
    pentool::scene::validate(&document).unwrap();
    let safe_svg =
        pentool::scene3d::pack::flat_dieline_svg(&document["dielines"]["sample-carton"]).unwrap();
    assert!(safe_svg.contains("data-id=\"safe-zone-front\" data-role=\"safe-zone\""));
    let safe_preflight = pentool::scene3d::pack::preflight(
        &document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    let front_safe_check = safe_preflight["safe_zone_checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["panel"] == "front")
        .unwrap();
    assert_eq!(front_safe_check["inset_mm"], 25.0);
    assert_eq!(front_safe_check["within_safe_zone"], false);
    assert!(safe_preflight["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap()
            .contains("outside its 25 mm safe area")));
    pentool::scene3d::bindings::set_panel_safe_zone(&mut document, "sample-carton", "front", 5.0)
        .unwrap();
    let inside_preflight = pentool::scene3d::pack::preflight(
        &document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    let front_inside = inside_preflight["safe_zone_checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["panel"] == "front")
        .unwrap();
    assert_eq!(front_inside["within_safe_zone"], true);
    assert_eq!(inside_preflight["panel_connected_components"], 1);
    assert_eq!(inside_preflight["design_geometry_status"], "checks-passed");
    assert_eq!(
        inside_preflight["fold_collision_report"]["status"],
        "checked-clear"
    );
    assert_eq!(
        inside_preflight["fold_collision_report"]["collision_count"],
        0
    );
    assert!(inside_preflight["production_path_checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|check| check["valid"] == true && check["within_panel_envelope"] == true));
    let mut malformed_production = document.clone();
    for panel in malformed_production["dielines"]["sample-carton"]["panels"]
        .as_array_mut()
        .unwrap()
    {
        panel.as_object_mut().unwrap().remove("artwork");
    }
    // Exercise legacy stored-path preflight separately from the editable-page
    // path validator, which correctly rejects malformed ordinary path nodes.
    malformed_production["dielines"]["sample-carton"]
        .as_object_mut()
        .unwrap()
        .remove("layout_page");
    malformed_production["dielines"]["sample-carton"]["paths"][0]["d"] =
        serde_json::json!("M invalid path");
    let malformed_preflight = pentool::scene3d::pack::preflight(
        &malformed_production,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    assert_eq!(
        malformed_preflight["design_geometry_status"],
        "needs-review"
    );
    assert_eq!(
        malformed_preflight["production_path_checks"][0]["valid"],
        false
    );
    assert!(malformed_preflight["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("malformed")));
    let mut raster_document = document.clone();
    pentool::raster::add(
        &mut raster_document,
        Some(&page_id),
        Some(&layer_id),
        "raster-art",
        0.0,
        0.0,
        32,
        32,
    )
    .unwrap();
    let raster_export = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &raster_document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    assert_eq!(raster_export.rasterized_panels, ["front"]);
    assert!(raster_export.svg.contains("data:image/png;base64,"));
    let raster_preview = pentool::render::svg_to_png(&raster_export.svg, 800, 900, 1.0).unwrap();
    assert!(raster_preview.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(raster_export.rasterized_ppi[0]["panel"], "front");
    assert!(
        raster_export.rasterized_ppi[0]["effective_ppi"]
            .as_f64()
            .unwrap()
            < 300.0
    );
    assert!(raster_export
        .warnings
        .iter()
        .any(|warning| warning.contains("below --min-ppi")));
    let preflight = pentool::scene3d::pack::preflight(
        &raster_document,
        std::path::Path::new("."),
        "sample-carton",
        300.0,
    )
    .unwrap();
    assert_eq!(preflight["preflight_status"], "needs-review");
    assert_eq!(preflight["rasterized_panels"][0], "front");
    assert!(preflight["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("below --min-ppi")));
    assert_eq!(preflight["manufacturing_certified"], false);
    let node = document["pages"][0]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let folded =
        pentool::scene3d::node::render_png(&document, std::path::Path::new("."), node).unwrap();
    let mut flat_document = document.clone();
    flat_document["dielines"]["sample-carton"]["fold_state"] = serde_json::json!(0.0);
    let flat_node = flat_document["pages"][0]["layers"][0]["nodes"][1]
        .as_object()
        .unwrap();
    let flat =
        pentool::scene3d::node::render_png(&flat_document, std::path::Path::new("."), flat_node)
            .unwrap();
    assert!(folded.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_ne!(folded, flat, "carton fold should change the CPU render");

    let mut mailer_document = document.clone();
    pentool::scene3d::pack::add_tuck_carton(
        &mut mailer_document,
        "sample-mailer",
        "sample-mailer-preview",
        &layer_id,
        Some(&page_id),
        "mailer",
        180.0,
        120.0,
        35.0,
        0.6,
        3.0,
        18.0,
        [20.0, 20.0, 240.0, 200.0],
    )
    .unwrap();
    pentool::scene::validate(&mailer_document).unwrap();
    let mailer_node = mailer_document["pages"][0]["layers"][0]["nodes"][2]
        .as_object()
        .unwrap();
    let mailer_folded = pentool::scene3d::node::render_png(
        &mailer_document,
        std::path::Path::new("."),
        mailer_node,
    )
    .unwrap();
    let mut mailer_flat_document = mailer_document.clone();
    mailer_flat_document["dielines"]["sample-mailer"]["fold_state"] = serde_json::json!(0.0);
    let mailer_flat_node = mailer_flat_document["pages"][0]["layers"][0]["nodes"][2]
        .as_object()
        .unwrap();
    let mailer_flat = pentool::scene3d::node::render_png(
        &mailer_flat_document,
        std::path::Path::new("."),
        mailer_flat_node,
    )
    .unwrap();
    assert!(mailer_folded.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_ne!(
        mailer_folded, mailer_flat,
        "mailer fold should change the CPU render"
    );
}

#[test]
fn packaging_cli_is_dry_run_safe_and_commits_renderable_dielines() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pentool-v0160-sleeve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("sleeve.pen");
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(320, 240)).unwrap();
    let mut v11 = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    let mut artwork_page = v11["pages"][0].clone();
    artwork_page["id"] = serde_json::json!("artwork");
    artwork_page["name"] = serde_json::json!("Label artwork");
    artwork_page["layers"][0]["id"] = serde_json::json!("artwork-layer");
    artwork_page["layers"][0]["name"] = serde_json::json!("Artwork");
    v11["pages"].as_array_mut().unwrap().push(artwork_page);
    v11["layout"]["spreads"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":"spread-artwork","pages":["artwork"],"facing":false}));
    std::fs::write(&input, serde_json::to_vec_pretty(&v11).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let args = [
        "scene3d",
        input.to_str().unwrap(),
        "--dry-run",
        "pack",
        "sleeve",
        "--dieline",
        "sleeve-1",
        "--node",
        "sleeve-preview",
        "--layer",
        "layer-1",
        "--length-mm",
        "120",
        "--width-mm",
        "70",
        "--height-mm",
        "180",
    ];
    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let committed = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "sleeve",
            "--dieline",
            "sleeve-1",
            "--node",
            "sleeve-preview",
            "--layer",
            "layer-1",
            "--length-mm",
            "120",
            "--width-mm",
            "70",
            "--height-mm",
            "180",
        ])
        .output()
        .unwrap();
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    pentool::scene::validate(&stored).unwrap();
    assert_eq!(
        stored["dielines"]["sleeve-1"]["panels"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        stored["pages"][0]["layers"][0]["nodes"][0]["dieline"],
        "sleeve-1"
    );
    let page_id = stored["pages"][0]["id"].as_str().unwrap();
    let linked = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "bind",
            "sleeve-1",
            "--panel",
            "front",
            "--source",
            &format!("page:{page_id}"),
        ])
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        stored["dielines"]["sleeve-1"]["panels"][0]["artwork"]["kind"],
        "page"
    );
    let before_carton = std::fs::read(&input).unwrap();
    let carton_args = [
        "scene3d",
        input.to_str().unwrap(),
        "--dry-run",
        "pack",
        "carton",
        "--dieline",
        "carton-1",
        "--node",
        "carton-preview",
        "--layer",
        "layer-1",
        "--template",
        "straight-tuck",
        "--length-mm",
        "120",
        "--width-mm",
        "70",
        "--height-mm",
        "180",
    ];
    let carton_dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(carton_args)
        .output()
        .unwrap();
    assert!(
        carton_dry.status.success(),
        "{}",
        String::from_utf8_lossy(&carton_dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before_carton);
    let carton_commit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "carton",
            "--dieline",
            "carton-1",
            "--node",
            "carton-preview",
            "--layer",
            "layer-1",
            "--template",
            "reverse-tuck",
            "--length-mm",
            "120",
            "--width-mm",
            "70",
            "--height-mm",
            "180",
        ])
        .output()
        .unwrap();
    assert!(
        carton_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&carton_commit.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(stored["dielines"]["carton-1"]["template"], "reverse-tuck");
    assert_eq!(
        stored["dielines"]["carton-1"]["panels"]
            .as_array()
            .unwrap()
            .len(),
        13
    );
    let before_crease = std::fs::read(&input).unwrap();
    let crease_args = [
        "scene3d",
        input.to_str().unwrap(),
        "--dry-run",
        "pack",
        "crease",
        "carton-1",
        "front-body-fold",
        "--angle-deg",
        "-45",
    ];
    let crease_dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(crease_args)
        .output()
        .unwrap();
    assert!(
        crease_dry.status.success(),
        "{}",
        String::from_utf8_lossy(&crease_dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before_crease);
    let crease_commit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "crease",
            "carton-1",
            "front-body-fold",
            "--angle-deg",
            "-45",
        ])
        .output()
        .unwrap();
    assert!(
        crease_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&crease_commit.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        stored["dielines"]["carton-1"]["creases"][0]["angle_deg"],
        -45.0
    );
    let before_direction = std::fs::read(&input).unwrap();
    let direction_dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--dry-run",
            "pack",
            "crease",
            "carton-1",
            "front-body-fold",
            "--direction",
            "mountain",
        ])
        .output()
        .unwrap();
    assert!(
        direction_dry.status.success(),
        "{}",
        String::from_utf8_lossy(&direction_dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before_direction);
    let direction_commit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "crease",
            "carton-1",
            "front-body-fold",
            "--direction",
            "mountain",
        ])
        .output()
        .unwrap();
    assert!(
        direction_commit.status.success(),
        "{}",
        String::from_utf8_lossy(&direction_commit.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        stored["dielines"]["carton-1"]["creases"][0]["direction"],
        "mountain"
    );
    let settings_commit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "settings",
            "carton-1",
            "--board-mm",
            "1.2",
            "--bleed-mm",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        settings_commit.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&settings_commit.stdout),
        String::from_utf8_lossy(&settings_commit.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        stored["dielines"]["carton-1"]["dimensions_mm"]["board"],
        1.2
    );
    assert_eq!(
        stored["dielines"]["carton-1"]["dimensions_mm"]["bleed"],
        5.0
    );
    let preflight = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "preflight",
            "carton-1",
            "--min-ppi",
            "300",
        ])
        .output()
        .unwrap();
    assert!(
        preflight.status.success(),
        "{}",
        String::from_utf8_lossy(&preflight.stderr)
    );
    let preflight_report: Value = serde_json::from_slice(&preflight.stdout).unwrap();
    assert_eq!(preflight_report["preflight_status"], "needs-review");
    assert_eq!(preflight_report["manufacturing_certified"], false);
    assert_eq!(
        preflight_report["panels_without_artwork"]
            .as_array()
            .unwrap()
            .len(),
        13
    );
    let before_preview = std::fs::read(&input).unwrap();
    let flat_preview_path = dir.join("carton-flat-preview.png");
    let folded_preview_path = dir.join("carton-folded-preview.png");
    for (fold, output_path) in [("0", &flat_preview_path), ("0.75", &folded_preview_path)] {
        let preview = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args([
                "scene3d",
                input.to_str().unwrap(),
                "pack",
                "preview",
                "carton-1",
                "--fold",
                fold,
                "--out",
                output_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        let report: Value = serde_json::from_slice(&preview.stdout).unwrap();
        assert_eq!(report["renderer"], "cpu-reference");
        assert_eq!(report["document_modified"], false);
        let png = std::fs::read(output_path).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
    assert_eq!(std::fs::read(&input).unwrap(), before_preview);
    assert_ne!(
        std::fs::read(flat_preview_path).unwrap(),
        std::fs::read(folded_preview_path).unwrap(),
        "fold amount must affect the CPU preview"
    );
    let templates = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["scene3d", input.to_str().unwrap(), "pack", "templates"])
        .output()
        .unwrap();
    assert!(templates.status.success());
    let template_report: Value = serde_json::from_slice(&templates.stdout).unwrap();
    let template_ids: Vec<&str> = template_report["templates"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|template| template["id"].as_str())
        .collect();
    assert_eq!(
        template_ids,
        [
            "sleeve",
            "straight-tuck",
            "reverse-tuck",
            "mailer",
            "pouch",
            "bottle-label",
            "tube-label",
            "svg-import"
        ]
    );
    let tube = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "tube",
            "--dieline",
            "tube-1",
            "--node",
            "tube-preview",
            "--layer",
            "layer-1",
            "--radius-mm",
            "30",
            "--height-mm",
            "140",
            "--overlap-mm",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        tube.status.success(),
        "{}",
        String::from_utf8_lossy(&tube.stderr)
    );
    let tube_report: Value = serde_json::from_slice(&tube.stdout).unwrap();
    assert_eq!(tube_report["template"], "tube");
    let tube_bind = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "bind",
            "tube-1",
            "--panel",
            "tube-label",
            "--source",
            "page:artwork",
        ])
        .output()
        .unwrap();
    assert!(
        tube_bind.status.success(),
        "{}",
        String::from_utf8_lossy(&tube_bind.stderr)
    );
    let tube_flat = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "export",
            "tube-1",
            "--out",
            dir.join("tube-flat.svg").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        tube_flat.status.success(),
        "{}",
        String::from_utf8_lossy(&tube_flat.stderr)
    );
    let tube_flat_svg = std::fs::read_to_string(dir.join("tube-flat.svg")).unwrap();
    assert!(tube_flat_svg.contains("data-role=\"wrap-seam-preview\""));
    assert!(tube_flat_svg.contains("data-role=\"wrap-overlap-preview\""));
    let tube_preflight = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "preflight",
            "tube-1",
            "--min-ppi",
            "36",
        ])
        .output()
        .unwrap();
    assert!(
        tube_preflight.status.success(),
        "{}",
        String::from_utf8_lossy(&tube_preflight.stderr)
    );
    let tube_preflight: Value = serde_json::from_slice(&tube_preflight.stdout).unwrap();
    assert_eq!(tube_preflight["wrap_seam_checks"][0]["touches_seam"], true);
    assert!(tube_preflight["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("wrap seam")));
    let tube_document: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert!(
        (tube_document["dielines"]["tube-1"]["panels"][0]["rect_mm"][2]
            .as_f64()
            .unwrap()
            - (std::f64::consts::TAU * 30.0 + 5.0))
            .abs()
            < 1e-4
    );
    let tube_preview_path = dir.join("tube-preview.png");
    let tube_preview = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "preview",
            "tube-1",
            "--out",
            tube_preview_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        tube_preview.status.success(),
        "{}",
        String::from_utf8_lossy(&tube_preview.stderr)
    );
    assert!(std::fs::read(tube_preview_path)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    let mailer = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "carton",
            "--dieline",
            "mailer-1",
            "--node",
            "mailer-preview",
            "--layer",
            "layer-1",
            "--template",
            "mailer",
            "--length-mm",
            "180",
            "--width-mm",
            "120",
            "--height-mm",
            "35",
            "--glue-flap-mm",
            "18",
        ])
        .output()
        .unwrap();
    assert!(
        mailer.status.success(),
        "{}",
        String::from_utf8_lossy(&mailer.stderr)
    );
    let after_mailer: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(after_mailer["dielines"]["mailer-1"]["template"], "mailer");
    assert_eq!(
        after_mailer["dielines"]["mailer-1"]["panels"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    let mut inspect_document: Value =
        serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    pentool::scene3d::bindings::set_panel_safe_zone(
        &mut inspect_document,
        "carton-1",
        "front",
        4.5,
    )
    .unwrap();
    std::fs::write(
        &input,
        serde_json::to_vec_pretty(&inspect_document).unwrap(),
    )
    .unwrap();
    let inspected = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "inspect",
            "carton-1",
        ])
        .output()
        .unwrap();
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let inspect_report: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspect_report["panel_count"], 13);
    assert_eq!(inspect_report["crease_count"], 12);
    assert_eq!(inspect_report["panel_art_bound"], 0);
    assert_eq!(inspect_report["panels"][0]["safe_zone_mm"], 4.5);
    assert_eq!(
        inspect_report["panels"][0]["artwork_transform"],
        Value::Null
    );
    assert!(inspect_report["paths"][0].get("d").is_none());
    let root_inspected = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", input.to_str().unwrap(), "inspect", "carton-1"])
        .output()
        .unwrap();
    assert!(
        root_inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&root_inspected.stderr)
    );
    let root_inspect_report: Value = serde_json::from_slice(&root_inspected.stdout).unwrap();
    assert_eq!(root_inspect_report["panels"][0]["safe_zone_mm"], 4.5);
    let templates = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "templates"])
        .output()
        .unwrap();
    assert!(
        templates.status.success(),
        "{}",
        String::from_utf8_lossy(&templates.stderr)
    );
    let template_report: Value = serde_json::from_slice(&templates.stdout).unwrap();
    assert!(template_report["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["id"] == "tube-label"));
    assert!(
        template_report["templates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|template| template["id"] == "svg-import"
                && template["status"] == "supported-subset")
    );
    let before_root_dry_run = std::fs::read(&input).unwrap();
    let root_dry_run = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            input.to_str().unwrap(),
            "--dry-run",
            "sleeve",
            "--dieline",
            "root-sleeve",
            "--node",
            "root-sleeve-preview",
            "--layer",
            "layer-1",
            "--length-mm",
            "120",
            "--width-mm",
            "70",
            "--height-mm",
            "180",
        ])
        .output()
        .unwrap();
    assert!(
        root_dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&root_dry_run.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before_root_dry_run);
    let flat_svg = dir.join("carton-flat.svg");
    let exported = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "pack",
            "export",
            "carton-1",
            "--out",
            flat_svg.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let svg = std::fs::read_to_string(flat_svg).unwrap();
    assert!(svg.starts_with("<?xml"));
    assert!(svg.contains("data-panel=\"front\""));
    assert!(svg.contains("data-role=\"cut\""));
    assert!(svg.contains("data-role=\"crease\""));
    let report: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(report["panel_art_included"], false);
    assert_eq!(report["manufacturing_certified"], false);
    let export_revision = pentool::transaction::revision(&std::fs::read(&input).unwrap());
    assert_eq!(report["revision"], export_revision);

    let stale_output = dir.join("stale.svg");
    let stale = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--if-revision",
            "stale-revision",
            "pack",
            "export",
            "carton-1",
            "--out",
            stale_output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("revision mismatch"));
    assert!(
        !stale_output.exists(),
        "stale export must not write an artifact"
    );
}

#[test]
fn product_form_cli_add_is_transactional_and_renderable() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pentool-v0160-form-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("form.pen");
    let output = dir.join("form.png");
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(128, 128)).unwrap();
    let v11 = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    std::fs::write(&input, serde_json::to_vec_pretty(&v11).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let args = [
        "scene3d",
        input.to_str().unwrap(),
        "--dry-run",
        "form",
        "add",
        "carton-node",
        "--form",
        "carton-form",
        "--kind",
        "box",
        "--parameters",
        r#"{"length_mm":50,"width_mm":30,"height_mm":70,"bevel_mm":1}"#,
        "--slots",
        "front,back,sides,top,bottom",
        "--layer",
        "layer-1",
        "--width",
        "80",
        "--height",
        "100",
    ];
    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let committed = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "form",
            "add",
            "carton-node",
            "--form",
            "carton-form",
            "--kind",
            "box",
            "--parameters",
            r#"{"length_mm":50,"width_mm":30,"height_mm":70,"bevel_mm":1}"#,
            "--slots",
            "front,back,sides,top,bottom",
            "--layer",
            "layer-1",
            "--width",
            "80",
            "--height",
            "100",
        ])
        .output()
        .unwrap();
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    let stored: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(stored["version"], 11);
    assert_eq!(
        stored["product_forms"]["carton-form"]["parameters"]["bevel_mm"],
        1
    );
    let listed = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["scene3d", input.to_str().unwrap(), "form", "presets"])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let catalog: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(catalog["presets"].as_array().unwrap().len(), 8);
    let starter = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "form",
            "add",
            "bottle-node",
            "--form",
            "demo-bottle",
            "--preset",
            "perfume-bottle",
            "--layer",
            "layer-1",
            "--width",
            "80",
            "--height",
            "100",
        ])
        .output()
        .unwrap();
    assert!(
        starter.status.success(),
        "{}",
        String::from_utf8_lossy(&starter.stderr)
    );
    let with_starter: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        with_starter["product_forms"]["demo-bottle"]["kind"],
        "lathe-bottle"
    );
    assert_eq!(
        with_starter["product_forms"]["demo-bottle"]["material_slots"][2],
        "cap"
    );
    let before_edit = std::fs::read(&input).unwrap();
    let dry_edit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "--dry-run",
            "form",
            "set",
            "carton-form",
            "--parameters",
            r#"{"length_mm":60,"width_mm":35,"height_mm":80,"bevel_mm":2}"#,
        ])
        .output()
        .unwrap();
    assert!(
        dry_edit.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_edit.stderr)
    );
    assert_eq!(
        std::fs::read(&input).unwrap(),
        before_edit,
        "form set dry-run must not publish"
    );
    let dry_edit_report: Value = serde_json::from_slice(&dry_edit.stdout).unwrap();
    let edited = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "form",
            "set",
            "carton-form",
            "--parameters",
            r#"{"length_mm":60,"width_mm":35,"height_mm":80,"bevel_mm":2}"#,
        ])
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let edited_report: Value = serde_json::from_slice(&edited.stdout).unwrap();
    assert_eq!(dry_edit_report["report"], edited_report["report"]);
    let edited_doc: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        edited_doc["product_forms"]["carton-form"]["parameters"]["length_mm"],
        60
    );
    let before_bad_edit = std::fs::read(&input).unwrap();
    let bad_edit = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "form",
            "set",
            "carton-form",
            "--parameters",
            r#"{"length_mm":60}"#,
        ])
        .output()
        .unwrap();
    assert!(!bad_edit.status.success());
    assert_eq!(std::fs::read(&input).unwrap(), before_bad_edit);
    let rendered = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["export", input.to_str().unwrap(), output.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    assert!(std::fs::metadata(output).unwrap().len() > 32);
}

#[test]
fn graphic_cli_add_set_list_remove_is_transactional() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pentool-v0160-graphic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("graphics.pen");
    let document = fixture();
    std::fs::write(&input, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let add_args = [
        "scene3d",
        input.to_str().unwrap(),
        "--dry-run",
        "graphic",
        "perfume-preview",
        "add",
        "season-label",
        "--source",
        "page:artwork",
        "--slot",
        "label-band",
        "--mode",
        "decal",
        "--fit",
        "contain",
        "--scale",
        "0.8,0.3",
        "--offset",
        "0.5,0.48",
        "--rotate",
        "4",
        "--opacity",
        "0.9",
    ];
    let dry = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(add_args)
        .output()
        .unwrap();
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let add = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "add",
            "season-label",
            "--source",
            "node:artwork:label-name",
            "--slot",
            "label-band",
            "--mode",
            "decal",
            "--projection",
            "cylindrical",
            "--axis",
            "y",
            "--scale",
            "0.7,0.2",
        ])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let after_add: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let added_graphic = after_add["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "season-label")
        .unwrap();
    assert_eq!(added_graphic["source"]["node"], "label-name");
    assert_eq!(added_graphic["axis"], "y");
    let undo = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["undo", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        undo.status.success(),
        "{}",
        String::from_utf8_lossy(&undo.stderr)
    );
    let undone: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert!(
        !undone["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "season-label"),
        "undo must remove the committed binding"
    );
    let redo = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["redo", input.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        redo.status.success(),
        "{}",
        String::from_utf8_lossy(&redo.stderr)
    );
    let reopened: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert_eq!(
        reopened["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"],
        after_add["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"],
        "redo and reload must preserve the exact binding record"
    );
    let list = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "list",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let listing: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert!(listing["surface_graphics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == "season-label"));
    let set = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "set",
            "season-label",
            "--opacity",
            "0.65",
            "--rotate",
            "-8",
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let after_set: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let edited_graphic = after_set["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "season-label")
        .unwrap();
    assert_eq!(edited_graphic["opacity"], 0.65);
    let invalid_before = std::fs::read(&input).unwrap();
    let invalid = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "set",
            "season-label",
            "--opacity",
            "3",
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(std::fs::read(&input).unwrap(), invalid_before);
    let remove = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "remove",
            "season-label",
        ])
        .output()
        .unwrap();
    assert!(
        remove.status.success(),
        "{}",
        String::from_utf8_lossy(&remove.stderr)
    );
    let after_remove: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert!(
        !after_remove["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "season-label")
    );
    let fill = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "add",
            "pattern-fill",
            "--source",
            "page:artwork",
            "--slot",
            "label-band",
            "--mode",
            "fill",
            "--fit",
            "cover",
            "--wrap",
            "mirror",
            "--scale",
            "2,3",
            "--offset",
            "0.25,0.5",
            "--crop",
            "0.1,0.1,0.5,0.8",
        ])
        .output()
        .unwrap();
    assert!(
        fill.status.success(),
        "{}",
        String::from_utf8_lossy(&fill.stderr)
    );
    let after_fill: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    let fill_record = after_fill["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "pattern-fill")
        .unwrap();
    assert_eq!(fill_record["mode"], "fill");
    assert_eq!(fill_record["fit"], "cover");
    assert_eq!(fill_record["wrap"], "mirror");
    assert_eq!(fill_record["scale"], serde_json::json!([2.0, 3.0]));
    assert_eq!(fill_record["offset"], serde_json::json!([0.25, 0.5]));
    assert_eq!(fill_record["crop"], serde_json::json!([0.1, 0.1, 0.5, 0.8]));
    let fill_render = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "render-report",
            "perfume-preview",
        ])
        .output()
        .unwrap();
    assert!(
        fill_render.status.success(),
        "{}",
        String::from_utf8_lossy(&fill_render.stderr)
    );
    let fill_render_report: Value = serde_json::from_slice(&fill_render.stdout).unwrap();
    assert_eq!(fill_render_report["node"], "perfume-preview");
    assert!(
        fill_render_report["render"]["output"]["width_px"]
            .as_u64()
            .unwrap()
            > 0
    );
    let remove_fill = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "graphic",
            "perfume-preview",
            "remove",
            "pattern-fill",
        ])
        .output()
        .unwrap();
    assert!(
        remove_fill.status.success(),
        "{}",
        String::from_utf8_lossy(&remove_fill.stderr)
    );
    let after_fill_remove: Value = serde_json::from_slice(&std::fs::read(&input).unwrap()).unwrap();
    assert!(
        !after_fill_remove["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "pattern-fill")
    );
}

#[test]
fn unknown_material_slot_is_a_stable_diagnostic_and_leaves_the_document_unchanged() {
    use std::process::Command;
    // Document validation names the slot, the consumer and the corrective action.
    let mut document = fixture();
    document["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"][0]["slot"] =
        Value::String("no-such-slot".into());
    let error = pentool::scene::validate(&document).unwrap_err().to_string();
    assert!(error.contains("[missing-resource]"), "{error}");
    assert!(error.contains("material slot `no-such-slot`"), "{error}");

    // The CLI mutation path refuses the same slot and does not touch the file.
    let dir = std::env::temp_dir().join(format!("pentool-v0160-slot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("slot.pen");
    std::fs::write(&input, serde_json::to_vec_pretty(&fixture()).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    for dry in [true, false] {
        let mut args = vec!["scene3d", input.to_str().unwrap()];
        if dry {
            args.push("--dry-run");
        }
        args.extend([
            "graphic",
            "perfume-preview",
            "add",
            "bad-slot",
            "--source",
            "page:artwork",
            "--slot",
            "no-such-slot",
        ]);
        let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(&args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "unknown slot must be refused");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[missing-resource]") && stderr.contains("no-such-slot"),
            "{stderr}"
        );
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uv_layout_on_a_dieline_preview_is_refused_not_a_panic() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pentool-v0160-dieline-uv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("dieline.pen");
    std::fs::write(&input, serde_json::to_vec_pretty(&fixture()).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let svg = dir.join("uv.svg");
    let out = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            input.to_str().unwrap(),
            "uv",
            "carton-preview",
            "layout",
            "--out",
            svg.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(stderr.contains("[unsupported-capability]"), "{stderr}");
    assert!(!svg.exists());
    assert_eq!(std::fs::read(&input).unwrap(), before);
    std::fs::remove_dir_all(dir).unwrap();
}
