use serde_json::Value;

#[test]
fn changing_crease_direction_updates_folded_geometry_without_changing_angle() {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let dieline = pentool::scene3d::pack::dieline_from_document(&document, "shirt-carton").unwrap();
    let valley_mesh = pentool::scene3d::pack::generate(&dieline).unwrap();
    let before = document.clone();
    let result = pentool::scene3d::pack::set_crease_direction(
        &mut document,
        "shirt-carton",
        "front-right-fold",
        "valley",
    )
    .unwrap();
    assert_eq!(result["previous_direction"], "mountain");
    assert_eq!(
        document["dielines"]["shirt-carton"]["creases"][1]["angle_deg"],
        90.0
    );
    let mountain_dieline =
        pentool::scene3d::pack::dieline_from_document(&document, "shirt-carton").unwrap();
    let mountain_mesh = pentool::scene3d::pack::generate(&mountain_dieline).unwrap();
    let positions = |model: &pentool::scene3d::model::Model| {
        model
            .primitives
            .iter()
            .map(|primitive| primitive.positions.clone())
            .collect::<Vec<_>>()
    };
    assert_ne!(positions(&valley_mesh), positions(&mountain_mesh));

    let mut invalid = before.clone();
    assert!(pentool::scene3d::pack::set_crease_direction(
        &mut invalid,
        "shirt-carton",
        "front-right-fold",
        "sideways",
    )
    .is_err());
    assert_eq!(invalid, before);
}

#[test]
fn generated_carton_preflight_keeps_editable_structural_creases_in_print_handoff() {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    pentool::scene3d::pack::add_tuck_carton(
        &mut document,
        "generated-carton",
        "generated-carton-preview",
        "mockups",
        Some("products"),
        "straight-tuck",
        120.0,
        70.0,
        180.0,
        0.5,
        3.0,
        15.0,
        [600.0, 100.0, 400.0, 600.0],
    )
    .unwrap();

    let dieline =
        pentool::scene3d::pack::dieline_from_document(&document, "generated-carton").unwrap();
    assert!(dieline["paths"]
        .as_array()
        .unwrap()
        .iter()
        .all(|path| path["role"] != "crease"));
    assert_eq!(dieline["creases"].as_array().unwrap().len(), 12);

    let preflight = pentool::scene3d::pack::preflight(
        &document,
        std::path::Path::new("generated-carton.pen"),
        "generated-carton",
        300.0,
    )
    .unwrap();
    assert!(!preflight["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning
            .as_str()
            .unwrap()
            .contains("crease production paths")));

    let flat = pentool::scene3d::pack::flat_dieline_svg(&dieline).unwrap();
    assert_eq!(flat.matches("data-role=\"crease\"").count(), 12);

    let dir = scratch();
    let plates = dir.join("vector-plates");
    let report = pentool::scene3d::pack::write_dieline_vector_spot_plates(
        &document,
        "generated-carton",
        &plates,
    )
    .unwrap();
    let crease_plate = report["plates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plate| plate["ink"] == "Crease")
        .unwrap();
    let crease_svg =
        std::fs::read_to_string(plates.join(crease_plate["path"].as_str().unwrap())).unwrap();
    assert_eq!(crease_svg.matches("data-role=\"crease\"").count(), 12);
}

#[test]
fn print_settings_update_flat_export_bounds_without_rebuilding_the_dieline() {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let before_resolved =
        pentool::scene3d::pack::dieline_from_document(&document, "shirt-carton").unwrap();
    let before_panels = document["dielines"]["shirt-carton"]["panels"].clone();
    let before_paths = document["dielines"]["shirt-carton"]["paths"].clone();
    let result = pentool::scene3d::pack::set_print_settings(
        &mut document,
        "shirt-carton",
        Some(1.2),
        Some(5.0),
        None,
        None,
    )
    .unwrap();
    assert_eq!(result["previous_board_mm"], 0.6);
    assert_eq!(result["previous_bleed_mm"], 3.0);
    assert_eq!(
        document["dielines"]["shirt-carton"]["panels"],
        before_panels
    );
    assert_eq!(document["dielines"]["shirt-carton"]["paths"], before_paths);
    let resolved =
        pentool::scene3d::pack::dieline_from_document(&document, "shirt-carton").unwrap();
    assert_eq!(resolved["panels"], before_resolved["panels"]);
    assert_eq!(resolved["paths"], before_resolved["paths"]);
    let svg = pentool::scene3d::pack::flat_dieline_svg(&resolved).unwrap();
    assert!(svg.contains("width=\"734mm\""));
    assert!(svg.contains("data-layout-bleed-mm=\"5\""));
    pentool::scene3d::pack::set_print_settings(
        &mut document,
        "shirt-carton",
        None,
        Some(0.0),
        None,
        None,
    )
    .unwrap();
    let returned =
        pentool::scene3d::pack::dieline_from_document(&document, "shirt-carton").unwrap();
    assert_eq!(returned["panels"], before_resolved["panels"]);
    assert_eq!(returned["paths"], before_resolved["paths"]);

    let mut bounded = document.clone();
    pentool::scene3d::pack::add_tuck_carton(
        &mut bounded,
        "bounded-carton",
        "bounded-carton-preview",
        "mockups",
        Some("products"),
        "straight-tuck",
        120.0,
        80.0,
        220.0,
        0.6,
        0.0,
        15.0,
        [0.0, 0.0, 100.0, 100.0],
    )
    .unwrap();
    let layout_page = bounded["dielines"]["bounded-carton"]["layout_page"]
        .as_str()
        .unwrap()
        .to_owned();
    bounded["pages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|page| page["id"] == layout_page)
        .unwrap()["canvas"]["width"] = serde_json::json!(16_384);
    let before_failed_edit = bounded.clone();
    assert!(pentool::scene3d::pack::set_print_settings(
        &mut bounded,
        "bounded-carton",
        None,
        Some(10.0),
        None,
        None,
    )
    .is_err());
    assert_eq!(bounded, before_failed_edit);
}

#[test]
fn cylindrical_wrap_settings_update_the_shared_flat_and_folded_mapping_atomically() {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    pentool::scene3d::pack::add_tube_label_with_overlap(
        &mut document,
        "tube-label",
        "tube-label-preview",
        "mockups",
        Some("products"),
        28.0,
        96.0,
        3.0,
        2.0,
        [25.0, 30.0, 260.0, 500.0],
    )
    .unwrap();
    let before = pentool::scene3d::pack::dieline_from_document(&document, "tube-label").unwrap();
    let before_model = pentool::scene3d::pack::generate(&before).unwrap();
    let original = document.clone();

    let result = pentool::scene3d::pack::set_print_settings(
        &mut document,
        "tube-label",
        None,
        None,
        Some(42.0),
        Some("counterclockwise"),
    )
    .unwrap();
    assert_eq!(result["previous_seam_angle_deg"], 0.0);
    assert_eq!(result["seam_angle_deg"], 42.0);
    assert_eq!(result["previous_wrap_direction"], "clockwise");
    assert_eq!(result["wrap_direction"], "counterclockwise");
    assert_eq!(
        document["dielines"]["tube-label"]["wrap"]["seam_angle_deg"],
        42.0
    );
    assert_eq!(
        document["dielines"]["tube-label"]["wrap"]["direction"],
        "counterclockwise"
    );

    let updated = pentool::scene3d::pack::dieline_from_document(&document, "tube-label").unwrap();
    let flat = pentool::scene3d::pack::flat_dieline_svg(&updated).unwrap();
    assert!(flat.contains("data-angle-deg=\"42\""));
    assert!(flat.contains("data-direction=\"counterclockwise\""));
    let updated_model = pentool::scene3d::pack::generate(&updated).unwrap();
    assert_ne!(
        before_model.primitives[0].positions,
        updated_model.primitives[0].positions
    );
    assert_eq!(
        document["dielines"]["tube-label"]["panels"],
        original["dielines"]["tube-label"]["panels"]
    );

    let before_invalid = document.clone();
    assert!(pentool::scene3d::pack::set_print_settings(
        &mut document,
        "tube-label",
        None,
        None,
        Some(361.0),
        None,
    )
    .is_err());
    assert_eq!(document, before_invalid);
    assert!(pentool::scene3d::pack::set_print_settings(
        &mut document,
        "shirt-carton",
        None,
        None,
        Some(15.0),
        None,
    )
    .is_err());
    assert_eq!(document, before_invalid);

    let dir = scratch();
    let path = dir.join("tube-wrap-settings.pen");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let cli = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            path.to_str().unwrap(),
            "settings",
            "tube-label",
            "--seam-angle-deg",
            "-30",
            "--wrap-direction",
            "clockwise",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(
        saved["dielines"]["tube-label"]["wrap"]["seam_angle_deg"],
        -30.0
    );
    assert_eq!(
        saved["dielines"]["tube-label"]["wrap"]["direction"],
        "clockwise"
    );
}

#[test]
fn assigning_a_layout_path_role_adds_it_to_the_exported_production_plate() {
    let mut document = serde_json::json!({
        "version":11,
        "pages":[{
            "id":"flat-net",
            "dieline_layout":{"dieline":"custom-box","coordinates":"mm"},
            "layers":[{"id":"production","nodes":[
                {"kind":"rect","id":"front-panel","dieline_panel_id":"front","x":0,"y":0,"width":100,"height":80},
                {"kind":"path","id":"new-trim","d":"M 5 5 L 30 5 L 30 30 L 5 30 Z"}
            ]}]
        }],
        "dielines":{"custom-box":{
            "template":"svg-import","units":"mm","layout_page":"flat-net",
            "dimensions_mm":{"length":100,"width":80,"height":80,"board":0.4,"bleed":0},
            "panels":[{"id":"front","face_slot":"front","rect_mm":[0,0,100,80],"uv_rect":[0,0,1,1]}],
            "creases":[],"paths":[]
        }}
    });
    let result = pentool::scene3d::bindings::set_layout_path_role(
        &mut document,
        "custom-box",
        "new-trim",
        "cut",
    )
    .unwrap();
    assert_eq!(result["spot"], "CutContour");
    assert_eq!(
        document["pages"][0]["layers"][0]["nodes"][1]["style"]["stroke"]["fallback"],
        "#dc2626"
    );

    let resolved = pentool::scene3d::pack::dieline_from_document(&document, "custom-box").unwrap();
    assert_eq!(resolved["paths"].as_array().unwrap().len(), 1);
    assert_eq!(resolved["paths"][0]["role"], "cut");
    assert_eq!(resolved["paths"][0]["d"], "M5,5 L30,5 L30,30 L5,30 Z");
    let svg = pentool::scene3d::pack::flat_dieline_svg(&resolved).unwrap();
    assert!(svg.contains("data-role=\"cut\""));
    assert!(svg.contains("data-spot=\"CutContour\""));

    document["pages"][0]["layers"][0]["nodes"][1]["d"] = serde_json::json!("M 5 5 Q 20 -5 35 5");
    let crease = pentool::scene3d::bindings::set_layout_path_role(
        &mut document,
        "custom-box",
        "new-trim",
        "crease",
    )
    .unwrap();
    assert_eq!(crease["role"], "crease");
    assert_eq!(crease["spot"], "Crease");
    assert_eq!(
        document["pages"][0]["layers"][0]["nodes"][1]["style"]["stroke"]["fallback"],
        "#2563eb"
    );
    let resolved = pentool::scene3d::pack::dieline_from_document(&document, "custom-box").unwrap();
    assert_eq!(resolved["paths"][0]["role"], "crease");
    let svg = pentool::scene3d::pack::flat_dieline_svg(&resolved).unwrap();
    assert!(svg.contains("data-role=\"crease\""));
    assert!(svg.contains("data-spot=\"Crease\""));
    assert!(svg.contains("Q20,-5 35,5"));

    let mut export_document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    export_document["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id":"custom-score", "role":"crease", "spot":"Crease",
            "d":"M 20 20 Q 40 10 60 20"
        }));
    let dir = scratch();
    let plates = dir.join("vector-plates");
    let report = pentool::scene3d::pack::write_dieline_vector_spot_plates(
        &export_document,
        "shirt-carton",
        &plates,
    )
    .unwrap();
    let crease_plate = report["plates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plate| plate["ink"] == "Crease")
        .unwrap();
    let crease_svg =
        std::fs::read_to_string(plates.join(crease_plate["path"].as_str().unwrap())).unwrap();
    assert!(crease_svg.contains("data-role=\"crease\""));
    assert!(crease_svg.contains("data-id=\"custom-score\""));
    let resolved =
        pentool::scene3d::pack::dieline_from_document(&export_document, "shirt-carton").unwrap();
    let export_svg = pentool::scene3d::pack::flat_dieline_svg(&resolved).unwrap();
    assert!(export_svg.contains("data-id=\"custom-score\""));

    let unchanged = document.clone();
    let error = pentool::scene3d::bindings::set_layout_path_role(
        &mut document,
        "custom-box",
        "new-trim",
        "not-a-production-role",
    )
    .unwrap_err();
    assert!(error.to_string().contains("new packaging paths support"));
    assert_eq!(
        document, unchanged,
        "unsupported role edits must not mutate input"
    );

    let mut locked = unchanged.clone();
    locked["pages"][0]["layers"][0]["locked"] = serde_json::json!(true);
    let locked_before = locked.clone();
    let error = pentool::scene3d::bindings::set_layout_path_role(
        &mut locked,
        "custom-box",
        "new-trim",
        "cut",
    )
    .unwrap_err();
    assert!(error.to_string().contains("locked"));
    assert_eq!(locked, locked_before, "locked paths must remain unchanged");
}
use std::{path::PathBuf, process::Command};

#[test]
fn strict_spot_preflight_follows_linked_artwork_into_3d_mockups() {
    use pentool::color::spot::{Ink, Library, Provenance};

    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "kind":"scene3d", "id":"nested-spot-mockup", "x":0, "y":0,
            "width":300, "height":180, "product_form":"perfume-bottle",
            "surface_graphics":[{
                "id":"nested-spot-label",
                "source":{"kind":"page","page":"spot-source"},
                "slot":"label-band", "mode":"decal", "projection":"uv"
            }]
        }));
    raw["pages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id":"spot-source", "name":"Spot source",
            "canvas":{"width":100,"height":80,"background":"#ffffffff"},
            "layers":[{"id":"spot-layer","name":"Spot","nodes":[{
                "kind":"rect", "id":"brand-gold-art", "x":0, "y":0,
                "width":100, "height":80,
                "style":{"fill":{
                    "spot":{"ink":"brand-gold","tint":1.0},
                    "fallback":{"space":"srgb","channels":[0.8,0.55,0.1]}
                }}
            }]}]
        }));
    let mut inks = Library::default();
    inks.insert(Ink {
        id: "brand-gold".into(),
        name: "Brand Gold".into(),
        aliases: vec![],
        alternate: serde_json::json!({"space":"srgb","channels":[0.8,0.55,0.1]}),
        provenance: Provenance {
            source: "v0.16 regression fixture".into(),
            publisher: None,
            revision: None,
            license: None,
        },
        redistribution: "allowed".into(),
    })
    .unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(inks).unwrap();

    let strict = pentool::color::preflight::run(
        &raw,
        std::path::Path::new("target/linked-spot-preflight.pen"),
        Some("products"),
        &pentool::color::preflight::Options {
            bleed_scale: None,
            reject_spot_flattening: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        strict["issues"].as_array().unwrap().iter().any(|issue| {
            issue["code"] == "scene3d-spot-flattened"
                && issue["node"] == "perfume-preview"
                && issue["severity"] == "error"
        }),
        "strict print policy must follow nested mockup bindings to spot art on another page: {strict}"
    );

    // Once the output page no longer binds artwork containing the nested
    // mockup, its remote spot source is unrelated and must not trip policy.
    raw["pages"][1]["layers"][0]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("surface_graphics");
    for panel in raw["dielines"]["shirt-carton"]["panels"]
        .as_array_mut()
        .unwrap()
    {
        panel.as_object_mut().unwrap().remove("artwork");
    }
    let unrelated = pentool::color::preflight::run(
        &raw,
        std::path::Path::new("target/linked-spot-preflight.pen"),
        Some("products"),
        &pentool::color::preflight::Options {
            bleed_scale: None,
            reject_spot_flattening: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!unrelated["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| { issue["code"] == "scene3d-spot-flattened" }));
}

#[test]
fn editable_panel_geometry_drives_flat_print_and_folded_mesh_from_one_map() {
    let document = serde_json::json!({
        "version":11,
        "pages":[{
            "id":"flat-net",
            "name":"Flat net",
            "dieline_layout":{"dieline":"editable-carton"},
            "canvas":{"width":30,"height":10,"background":"#ffffffff"},
            "layers":[{"id":"layout","name":"Layout","nodes":[
                {"kind":"rect","id":"front-object","dieline_panel_id":"front","x":0,"y":0,"width":10,"height":10,"style":{"fill":{"fallback":"none"}}},
                {"kind":"rect","id":"back-object","dieline_panel_id":"back","x":10,"y":0,"width":10,"height":10,"style":{"fill":{"fallback":"none"}}},
                {"kind":"path","id":"hinge-object","dieline_crease_id":"hinge","d":"M 10 0 L 10 10","style":{"fill":{"fallback":"none"}}}
            ]}]
        }],
        "dielines":{"editable-carton":{
            "template":"svg-import","units":"mm","fold_state":1.0,
            "dimensions_mm":{"length":20,"width":10,"height":10,"board":0,"bleed":0},
            "layout_page":"flat-net",
            "panels":[
                {"id":"front","face_slot":"front","rect_mm":[0,0,10,10],"uv_rect":[0,0,1,1]},
                {"id":"back","face_slot":"back","rect_mm":[10,0,10,10],"uv_rect":[0,0,1,1]}
            ],
            "creases":[{"id":"hinge","line_mm":[[10,0],[10,10]],"angle_deg":90,"direction":"valley"}],
            "paths":[]
        }}
    });
    let baseline =
        pentool::scene3d::pack::dieline_from_document(&document, "editable-carton").unwrap();
    let baseline_mesh = pentool::scene3d::pack::generate(&baseline).unwrap();
    let original_width = baseline["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == "front")
        .unwrap()["rect_mm"][2]
        .as_f64()
        .unwrap();
    let mut edited_document = document.clone();
    edited_document["pages"][0]["layers"][0]["nodes"][0]["width"] =
        serde_json::json!(original_width + 10.0);
    edited_document["pages"][0]["layers"][0]["nodes"][1]["x"] =
        serde_json::json!(original_width + 10.0);
    edited_document["pages"][0]["layers"][0]["nodes"][2]["d"] = serde_json::json!(format!(
        "M {} 0 L {} 10",
        original_width + 10.0,
        original_width + 10.0
    ));

    let edited =
        pentool::scene3d::pack::dieline_from_document(&edited_document, "editable-carton").unwrap();
    let edited_width = edited["panels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|panel| panel["id"] == "front")
        .unwrap()["rect_mm"][2]
        .as_f64()
        .unwrap();
    assert!((edited_width - (original_width + 10.0)).abs() < 1.0e-9);

    let edited_mesh = pentool::scene3d::pack::generate(&edited).unwrap();
    assert_ne!(
        baseline_mesh.primitives[0].positions, edited_mesh.primitives[0].positions,
        "folded face geometry must consume the edited panel map"
    );
    let flat_svg = pentool::scene3d::pack::flat_dieline_svg(&edited).unwrap();
    let panel_mark = flat_svg
        .find("data-panel=\"front\"")
        .map(|start| &flat_svg[start..(start + 300).min(flat_svg.len())])
        .expect("front panel print mark");
    assert!(
        panel_mark.contains(&format!("width=\"{edited_width}\"")),
        "flat print geometry must consume the same edited panel map: {panel_mark}"
    );
}

#[test]
fn non_full_panel_uv_rect_keeps_folded_linked_artwork_aligned_to_its_flat_panel() {
    let mut document: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let render_carton = |document: &Value| {
        let node = &document["pages"][1]["layers"][0]["nodes"][1];
        pentool::scene3d::node::render_png_in_page(
            document,
            std::path::Path::new("."),
            Some("products"),
            node.as_object().unwrap(),
        )
        .unwrap()
    };
    let full_uv_render = render_carton(&document);
    let flat_before = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &document,
        std::path::Path::new("."),
        "shirt-carton",
        300.0,
    )
    .unwrap();
    assert!(flat_before
        .svg
        .contains("<g id=\"panel-artwork\" transform=\"translate(3 3)\">"));

    document["dielines"]["shirt-carton"]["panels"][0]["uv_rect"] =
        serde_json::json!([0.2, 0.1, 0.6, 0.8]);
    pentool::scene::validate(&document).unwrap();
    let partial_uv_render = render_carton(&document);
    let flat_after = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &document,
        std::path::Path::new("."),
        "shirt-carton",
        300.0,
    )
    .unwrap();
    assert_eq!(
        flat_before.svg, flat_after.svg,
        "UV atlas placement must not move the flat print art"
    );
    let full_image = image::load_from_memory(&full_uv_render).unwrap().to_rgba8();
    let partial_image = image::load_from_memory(&partial_uv_render)
        .unwrap()
        .to_rgba8();
    assert_eq!(full_image.dimensions(), partial_image.dimensions());
    let mut different_pixels = 0_u64;
    let mut absolute_channel_delta = 0_u64;
    for (full, partial) in full_image.pixels().zip(partial_image.pixels()) {
        if full != partial {
            different_pixels += 1;
        }
        absolute_channel_delta += full
            .0
            .iter()
            .zip(partial.0.iter())
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum::<u64>();
    }
    let total_pixels = u64::from(full_image.width()) * u64::from(full_image.height());
    let differing_fraction = different_pixels as f64 / total_pixels as f64;
    let mean_channel_delta = absolute_channel_delta as f64 / (total_pixels * 4) as f64;
    assert!(
        differing_fraction < 0.02 && mean_channel_delta < 0.1,
        "UV atlas remapping should preserve the folded artwork within sampling tolerance; {different_pixels}/{total_pixels} pixels differ ({differing_fraction:.4}), mean channel delta {mean_channel_delta:.3}"
    );
}

#[test]
fn linked_spot_art_uses_its_fallback_color_in_cpu_mockup_rendering() {
    use pentool::color::spot::{Ink, Library, Provenance};

    let mut rendered = Vec::new();
    for channels in [[0.9, 0.1, 0.2], [0.1, 0.3, 0.9]] {
        let mut document: Value =
            serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        document["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"] = serde_json::json!({
            "spot":{"ink":"brand-test","tint":0.7},
            "fallback":{"space":"srgb","channels":channels}
        });
        let mut inks = Library::default();
        inks.insert(Ink {
            id: "brand-test".into(),
            name: "Brand Test".into(),
            aliases: vec![],
            alternate: serde_json::json!({"space":"srgb","channels":[0.4,0.4,0.4]}),
            provenance: Provenance {
                source: "v0.16 fallback regression".into(),
                publisher: None,
                revision: None,
                license: None,
            },
            redistribution: "allowed".into(),
        })
        .unwrap();
        document["color_management"]["inks"] = serde_json::to_value(inks).unwrap();
        pentool::scene::validate(&document).unwrap();
        let node = document["pages"][1]["layers"][0]["nodes"][0]
            .as_object()
            .unwrap();
        rendered.push(
            pentool::scene3d::node::render_png_in_page(
                &document,
                std::path::Path::new("."),
                Some("products"),
                node,
            )
            .unwrap(),
        );
    }
    assert_ne!(
        rendered[0], rendered[1],
        "changing a linked spot paint's sRGB fallback must change the deterministic mockup pixels"
    );
}

#[test]
fn linked_spot_fallback_reaches_flattened_pdfx_mockup_pixels() {
    use pentool::color::{
        icc::Intent,
        pdfx,
        spot::{Ink, Library, Provenance},
    };

    let mut outputs = Vec::new();
    let mut intent_outputs = Vec::new();
    let mut additional_intent_outputs = Vec::new();
    let mut icc_fallback_intent_outputs = Vec::new();
    let fallback_cases = [
        serde_json::json!({"space":"srgb","channels":[0.9,0.1,0.2]}),
        serde_json::json!({"space":"srgb","channels":[0.1,0.3,0.9]}),
        serde_json::json!({
            "space":"icc-cmyk", "profile":"press", "channels":[0.0,0.92,0.88,0.02]
        }),
    ];
    for (fallback_index, fallback) in fallback_cases.into_iter().enumerate() {
        let dir = scratch();
        let document_path = dir.join("spot-mockup.pen");
        let mut raw: Value =
            serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
                .unwrap();
        pentool::color::catalog::import(
            &mut raw,
            &document_path,
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
        let page_id = "products";
        raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"] = serde_json::json!({
            "spot":{"ink":"brand-test","tint":1.0},
            "fallback":fallback
        });
        raw["pages"][1]["production"] = serde_json::json!({
            "units_per_inch":72,
            "trim":{"x":0,"y":0,"width":1200,"height":800},
            "bleed":0
        });
        let mut inks = Library::default();
        inks.insert(Ink {
            id: "brand-test".into(),
            name: "Brand Test".into(),
            aliases: vec![],
            alternate: serde_json::json!({"space":"srgb","channels":[0.4,0.4,0.4]}),
            provenance: Provenance {
                source: "v0.16 PDF/X fallback regression".into(),
                publisher: None,
                revision: None,
                license: None,
            },
            redistribution: "allowed".into(),
        })
        .unwrap();
        raw["color_management"]["inks"] = serde_json::to_value(inks).unwrap();
        pentool::scene::validate(&raw).unwrap();
        if fallback_index == 2 {
            let mut missing_profile = raw.clone();
            missing_profile["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"]
                ["profile"] = serde_json::json!("not-a-profile");
            let error = pdfx::build(
                &missing_profile,
                &document_path,
                Some(page_id),
                &pdfx::Options {
                    ppi: 72.0,
                    created: Some("2026-10-15T00:00:00Z".into()),
                    title: Some("Spot fallback mockup".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
            let error = format!("{error:#}");
            assert!(
                error.contains("not-a-profile") && error.contains("ICC"),
                "a missing ICC fallback profile must fail with its identity: {error}"
            );
        }
        let exported = pdfx::build(
            &raw,
            &document_path,
            Some(page_id),
            &pdfx::Options {
                ppi: 72.0,
                created: Some("2026-10-15T00:00:00Z".into()),
                title: Some("Spot fallback mockup".into()),
                ..Default::default()
            },
        )
        .unwrap();
        pdfx::verify_structure(&exported.bytes).unwrap();
        outputs.push(exported.bytes);
        let perceptual = pdfx::build(
            &raw,
            &document_path,
            Some(page_id),
            &pdfx::Options {
                ppi: 72.0,
                intent: Some(Intent::Perceptual),
                created: Some("2026-10-15T00:00:00Z".into()),
                title: Some("Spot fallback mockup".into()),
                ..Default::default()
            },
        )
        .unwrap();
        intent_outputs.push(perceptual.bytes);
        if fallback_index == 0 {
            for intent in [Intent::Saturation, Intent::AbsoluteColorimetric] {
                let rendered = pdfx::build(
                    &raw,
                    &document_path,
                    Some(page_id),
                    &pdfx::Options {
                        ppi: 72.0,
                        intent: Some(intent),
                        created: Some("2026-10-15T00:00:00Z".into()),
                        title: Some("Spot fallback mockup".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
                pdfx::verify_structure(&rendered.bytes).unwrap();
                assert_eq!(
                    rendered.report["output_intent"]["intent"],
                    serde_json::json!(format!("{intent:?}")),
                    "the generated PDF/X report must identify the actual ICC intent"
                );
                additional_intent_outputs.push(rendered.bytes);
            }

            let mut managed_working = raw.clone();
            managed_working["color_management"]["roles"]["working"] = serde_json::json!({
                "destination":{"space":"display-p3:linear"},
                "intent":"relative-colorimetric"
            });
            let p3_working = pdfx::build(
                &managed_working,
                &document_path,
                Some(page_id),
                &pdfx::Options {
                    ppi: 72.0,
                    created: Some("2026-10-15T00:00:00Z".into()),
                    title: Some("Spot fallback mockup".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            pdfx::verify_structure(&p3_working.bytes).unwrap();
            assert_eq!(p3_working.report["pixels"]["channels"], 4);
            let srgb_working = pdfx::build(
                &raw,
                &document_path,
                Some(page_id),
                &pdfx::Options {
                    ppi: 72.0,
                    created: Some("2026-10-15T00:00:00Z".into()),
                    title: Some("Spot fallback mockup".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(
                p3_working.report["sha256"],
                srgb_working.report["sha256"],
                "colorimetrically equivalent RGB working spaces must preserve the same CMYK PDF/X pixels"
            );
        } else if fallback_index == 2 {
            for intent in [Intent::Saturation, Intent::AbsoluteColorimetric] {
                let rendered = pdfx::build(
                    &raw,
                    &document_path,
                    Some(page_id),
                    &pdfx::Options {
                        ppi: 72.0,
                        intent: Some(intent),
                        created: Some("2026-10-15T00:00:00Z".into()),
                        title: Some("ICC spot fallback mockup".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
                pdfx::verify_structure(&rendered.bytes).unwrap();
                assert_eq!(
                    rendered.report["output_intent"]["intent"],
                    serde_json::json!(format!("{intent:?}")),
                    "the ICC-fallback PDF/X report must identify the actual ICC intent"
                );
                icc_fallback_intent_outputs.push(rendered.bytes);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    assert_ne!(
        outputs[0], outputs[1],
        "PDF/X process pixels must use the linked spot paint's portable fallback"
    );
    assert_ne!(
        outputs[0], outputs[2],
        "an ICC-profiled linked spot fallback must reach the flattened PDF/X pixels"
    );
    assert_ne!(
        outputs[0], intent_outputs[0],
        "the declared PDF/X rendering intent must affect managed 3D output"
    );
    for output in additional_intent_outputs {
        assert_ne!(
            outputs[0], output,
            "each supported ICC intent must affect the managed 3D PDF/X pixels"
        );
    }
    for output in icc_fallback_intent_outputs {
        assert_ne!(
            outputs[2], output,
            "non-default ICC intents must affect the imported ICC-CMYK fallback pixels"
        );
    }
}

fn scratch() -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pentool-v0160-pouch-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn pack_bind_cli_is_dry_run_equivalent_revision_guarded_and_clearable() {
    let dir = scratch();
    let document = dir.join("bind.pen");
    std::fs::write(
        &document,
        include_bytes!("../docs/fixtures/v11-product-packaging.pen"),
    )
    .unwrap();
    let path = document.to_str().unwrap();
    let initial = std::fs::read(&document).unwrap();
    let initial_revision = pentool::transaction::revision(&initial);
    let source = "node:artwork:label-name";

    let dry_link = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            path,
            "--dry-run",
            "--if-revision",
            &initial_revision,
            "pack",
            "bind",
            "shirt-carton",
            "--panel",
            "front",
            "--source",
            source,
        ])
        .output()
        .unwrap();
    assert!(
        dry_link.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_link.stderr)
    );
    assert_eq!(std::fs::read(&document).unwrap(), initial);
    let dry_link_report: Value = serde_json::from_slice(&dry_link.stdout).unwrap();

    let linked = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            path,
            "--if-revision",
            &initial_revision,
            "pack",
            "bind",
            "shirt-carton",
            "--panel",
            "front",
            "--source",
            source,
        ])
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let link_report: Value = serde_json::from_slice(&linked.stdout).unwrap();
    for field in ["dieline", "panel", "artwork"] {
        assert_eq!(dry_link_report[field], link_report[field], "field {field}");
    }
    assert_eq!(link_report["artwork"]["kind"], "node");
    assert_eq!(link_report["artwork"]["node"], "label-name");

    let linked_bytes = std::fs::read(&document).unwrap();
    let stale = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            path,
            "--if-revision",
            &initial_revision,
            "pack",
            "bind",
            "shirt-carton",
            "--panel",
            "front",
            "--source",
            "page:artwork",
        ])
        .output()
        .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("revision mismatch"));
    assert_eq!(std::fs::read(&document).unwrap(), linked_bytes);

    let linked_revision = pentool::transaction::revision(&linked_bytes);
    let dry_clear = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            path,
            "--dry-run",
            "--if-revision",
            &linked_revision,
            "pack",
            "bind",
            "shirt-carton",
            "--panel",
            "front",
        ])
        .output()
        .unwrap();
    assert!(
        dry_clear.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_clear.stderr)
    );
    assert_eq!(std::fs::read(&document).unwrap(), linked_bytes);
    let dry_clear_report: Value = serde_json::from_slice(&dry_clear.stdout).unwrap();
    assert!(dry_clear_report["artwork"].is_null());

    let cleared = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "scene3d",
            path,
            "--if-revision",
            &linked_revision,
            "pack",
            "bind",
            "shirt-carton",
            "--panel",
            "front",
        ])
        .output()
        .unwrap();
    assert!(
        cleared.status.success(),
        "{}",
        String::from_utf8_lossy(&cleared.stderr)
    );
    let clear_report: Value = serde_json::from_slice(&cleared.stdout).unwrap();
    assert!(clear_report["artwork"].is_null());
    let after_clear: Value = serde_json::from_slice(&std::fs::read(&document).unwrap()).unwrap();
    assert!(after_clear["dielines"]["shirt-carton"]["panels"][0]
        .get("artwork")
        .is_none());
    pentool::scene::validate(&after_clear).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_vector_plates_refuse_to_merge_cut_and_crease_operations() {
    let dir = scratch();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let crease = raw["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|path| path["role"] == "crease")
        .unwrap();
    crease["spot"] = serde_json::json!("CutContour");

    let output = dir.join("plates");
    let error =
        pentool::scene3d::pack::write_dieline_vector_spot_plates(&raw, "shirt-carton", &output)
            .unwrap_err()
            .to_string();
    assert!(
        error.contains("assigned to both `cut` and `crease` roles"),
        "{error}"
    );
    assert!(
        !output.exists(),
        "invalid mixed-role spot assignment must fail before publishing plate files"
    );
    let raster_error = pentool::scene3d::pack::dieline_spot_separations(
        &raw,
        &dir.join("source.pen"),
        "shirt-carton",
        72.0,
    )
    .unwrap_err()
    .to_string();
    assert!(
        raster_error.contains("assigned to both `cut` and `crease` roles"),
        "{raster_error}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_preflight_warns_when_cut_paths_have_no_closed_contour() {
    let dir = scratch();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let cut = raw["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|path| path["role"] == "cut")
        .unwrap();
    cut["d"] = serde_json::json!("M 0 0 L 724 0");

    let report =
        pentool::scene3d::pack::preflight(&raw, &dir.join("source.pen"), "shirt-carton", 72.0)
            .unwrap();
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning.as_str().is_some_and(|warning| {
                    warning.contains("cut path `cut-outline` has no closed contour")
                })
            }),
        "{report}"
    );
    let path_report = report["production_path_checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|path| path["id"] == "cut-outline")
        .unwrap();
    assert_eq!(path_report["closed_subpaths"], 0);
    assert_eq!(report["design_geometry_status"], "needs-review");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_preflight_reports_pdfx_output_role_readiness_from_rust() {
    let source = std::path::Path::new("packaging-output-role.pen");
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let unconfigured =
        pentool::scene3d::pack::preflight(&raw, source, "shirt-carton", 72.0).unwrap();
    assert_eq!(unconfigured["pdfx_output_role"]["configured"], false);
    assert!(unconfigured["pdfx_output_role"]["message"]
        .as_str()
        .unwrap()
        .contains("output color role is not configured"));

    pentool::color::catalog::import(
        &mut raw,
        source,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["color_management"]["roles"]["output"] = serde_json::json!({
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    });
    let configured = pentool::scene3d::pack::preflight(&raw, source, "shirt-carton", 72.0).unwrap();
    assert_eq!(configured["pdfx_output_role"]["configured"], true);
    assert_eq!(configured["pdfx_output_role"]["space"], "icc-cmyk");
    assert_eq!(configured["pdfx_output_role"]["profile"], "press");
    assert_eq!(configured["pdfx_output_role"]["message"], Value::Null);
}

#[test]
fn packaging_cut_contour_panel_association_is_explicit_and_validated() {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let render_carton = |document: &Value| {
        pentool::scene3d::node::render_png_in_page(
            document,
            std::path::Path::new("."),
            Some("products"),
            document["pages"][1]["layers"][0]["nodes"][1]
                .as_object()
                .unwrap(),
        )
        .unwrap()
    };
    let rectangular_preview = render_carton(&raw);
    raw["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id":"front-art-trim",
            "role":"cut",
            "panel_id":"front",
            "d":"M 22 22 H 362 V 282 H 22 V 180 L 42 160 L 42 140 L 22 120 Z",
            "spot":"CutContour"
        }));
    pentool::scene::validate(&raw).unwrap();
    let trimmed_model = pentool::scene3d::pack::generate(&raw["dielines"]["shirt-carton"]).unwrap();
    let trimmed_front = trimmed_model
        .primitives
        .iter()
        .find(|primitive| primitive.name == "front")
        .unwrap();
    assert_eq!(trimmed_front.positions.len(), 8);
    assert_eq!(trimmed_front.indices.len(), 18);
    assert_eq!(trimmed_model.report.triangles, 38);
    assert_ne!(rectangular_preview, render_carton(&raw));
    let rendered = pentool::scene3d::pack::flat_dieline_svg_with_artwork(
        &raw,
        std::path::Path::new("."),
        "shirt-carton",
        72.0,
    )
    .unwrap();
    assert!(rendered.svg.contains("panel-art-cut-clip-0"));
    assert!(rendered.svg.contains("data-panel-bleed=\"front\""));
    assert_eq!(rendered.bleed_synthesized[0]["cut_path"], "front-art-trim");
    assert_eq!(
        rendered.bleed_synthesized[0]["method"],
        "nearest-contour pixel extension; closed panel cut contour"
    );
    let preflight =
        pentool::scene3d::pack::preflight(&raw, std::path::Path::new("."), "shirt-carton", 72.0)
            .unwrap();
    assert!(preflight["fold_hinge_checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["joined"] == true));

    let mut missing_panel = raw.clone();
    missing_panel["dielines"]["shirt-carton"]["paths"][2]["panel_id"] =
        serde_json::json!("missing");
    let error = pentool::scene::validate(&missing_panel)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("refers to missing panel `missing`"),
        "{error}"
    );

    let mut open_contour = raw;
    open_contour["dielines"]["shirt-carton"]["paths"][2]["d"] =
        serde_json::json!("M 32 32 H 352 V 272");
    let error = pentool::scene::validate(&open_contour)
        .unwrap_err()
        .to_string();
    assert!(error.contains("must contain a closed contour"), "{error}");
}

#[test]
fn packaging_preflight_reports_self_intersecting_edited_production_paths() {
    let dir = scratch();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    let cut = raw["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|path| path["role"] == "cut")
        .unwrap();
    cut["d"] = serde_json::json!("M 0 0 L 724 304 L 0 304 L 724 0 Z");

    let report =
        pentool::scene3d::pack::preflight(&raw, &dir.join("source.pen"), "shirt-carton", 72.0)
            .unwrap();
    assert!(
        report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning.as_str().is_some_and(|warning| {
                    warning.contains("production path `cut-outline` (cut) self-intersects")
                })
            }),
        "{report}"
    );
    let path_report = report["production_path_checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|path| path["id"] == "cut-outline")
        .unwrap();
    assert_eq!(path_report["self_intersects"], true);
    assert_eq!(report["design_geometry_status"], "needs-review");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_preview_and_export_dry_run_render_without_publishing_files() {
    let dir = scratch();
    let document = dir.join("packaging.pen");
    let mut document_value: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    pentool::color::catalog::import(
        &mut document_value,
        &document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    document_value["color_management"]["roles"] = serde_json::json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    }});
    std::fs::write(&document, serde_json::to_vec(&document_value).unwrap()).unwrap();
    let original = std::fs::read(&document).unwrap();
    let preview_path = dir.join("folded.png");
    let export_path = dir.join("flat.svg");
    let pdfx_path = dir.join("print.pdf");
    std::fs::write(&pdfx_path, b"preserve existing dry-run destination").unwrap();

    for (action, output) in [("preview", &preview_path), ("export", &export_path)] {
        let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args([
                "pack",
                document.to_str().unwrap(),
                "--dry-run",
                action,
                "shirt-carton",
                "--out",
                output.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["complete"], false);
        assert!(report["bytes"].as_u64().unwrap() > 0);
        assert!(!output.exists(), "dry-run published {}", output.display());
        assert_eq!(std::fs::read(&document).unwrap(), original);
    }

    let pdfx_dry_run = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            document.to_str().unwrap(),
            "--dry-run",
            "export",
            "shirt-carton",
            "--out",
            pdfx_path.to_str().unwrap(),
            "--pdfx",
            "--ppi",
            "36",
        ])
        .output()
        .unwrap();
    assert!(
        pdfx_dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&pdfx_dry_run.stderr)
    );
    let pdfx_report: Value = serde_json::from_slice(&pdfx_dry_run.stdout).unwrap();
    assert_eq!(pdfx_report["dry_run"], true);
    assert_eq!(pdfx_report["complete"], false);
    assert!(pdfx_report["bytes"].as_u64().unwrap() > 0);
    assert_eq!(
        std::fs::read(&pdfx_path).unwrap(),
        b"preserve existing dry-run destination"
    );
    assert_eq!(std::fs::read(&document).unwrap(), original);

    for (action, output) in [("preview", &preview_path), ("export", &export_path)] {
        let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args([
                "pack",
                document.to_str().unwrap(),
                action,
                "shirt-carton",
                "--out",
                output.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["dry_run"], false);
        assert_eq!(report["complete"], true);
        assert_eq!(
            report["bytes"].as_u64().unwrap(),
            std::fs::metadata(output).unwrap().len()
        );
    }

    assert_eq!(std::fs::read(&document).unwrap(), original);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packaging_print_bundle_separates_cut_and_crease_guides_from_artwork() {
    let dir = scratch();
    let document = dir.join("packaging-print.pen");
    let mut document_value: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/v11-product-packaging.pen"))
            .unwrap();
    document_value["dielines"]["shirt-carton"]["paths"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id":"tear-perforation", "role":"perforation",
            "spot":"Perforation", "d":"M 20 20 L 80 20"
        }));
    pentool::color::catalog::import(
        &mut document_value,
        &document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    document_value["color_management"]["roles"] = serde_json::json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},
        "intent":"relative-colorimetric"
    }});
    std::fs::write(&document, serde_json::to_vec(&document_value).unwrap()).unwrap();
    let original = std::fs::read(&document).unwrap();
    let revision = pentool::transaction::revision(&original);
    let bundle = dir.join("print-bundle");

    let dry_run = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            document.to_str().unwrap(),
            "--if-revision",
            &revision,
            "--dry-run",
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
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let dry_run_report: Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(dry_run_report["dry_run"], true);
    assert_eq!(dry_run_report["complete"], false);
    assert_eq!(dry_run_report["document_revision"], revision);
    assert!(
        dry_run_report["files"]["flat_svg"]["bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        dry_run_report["files"]["pdfx_draft"]["bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(!bundle.exists(), "dry-run published the print bundle");
    assert_eq!(std::fs::read(&document).unwrap(), original);

    let stale_revision = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            document.to_str().unwrap(),
            "--if-revision",
            "stale-revision",
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
    assert!(!stale_revision.status.success());
    assert!(String::from_utf8_lossy(&stale_revision.stderr).contains("revision mismatch"));
    assert!(
        !bundle.exists(),
        "stale revision published the print bundle"
    );
    assert_eq!(std::fs::read(&document).unwrap(), original);

    let export = Command::new(env!("CARGO_BIN_EXE_pentool"))
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
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let report: Value = serde_json::from_slice(&export.stdout).unwrap();
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["complete"], true);
    assert_eq!(report["manufacturing_certified"], false);
    assert!(bundle.join("flat.svg").is_file());
    assert!(bundle.join("CUTTING-MAP.svg").is_file());
    assert!(bundle.join("print-draft.pdf").is_file());
    assert!(bundle.join("folded-preview.png").is_file());
    let flat_svg = std::fs::read_to_string(bundle.join("flat.svg")).unwrap();
    let svg_root = flat_svg
        .split_once("<svg ")
        .and_then(|(_, rest)| rest.split_once('>').map(|(root, _)| root))
        .unwrap();
    let physical_mm = |axis: &str| {
        svg_root
            .split_once(&format!("{axis}=\""))
            .and_then(|(_, tail)| tail.split_once('"').map(|(value, _)| value))
            .and_then(|value| value.strip_suffix("mm"))
            .unwrap()
            .parse::<f64>()
            .unwrap()
    };
    let print_guide = std::fs::read_to_string(bundle.join("PRINT-GUIDE.txt")).unwrap();
    assert!(print_guide.contains("Check folded-preview.png first"));
    assert!(print_guide.contains("CUTTING-MAP.svg"));
    assert!(print_guide.contains("CutContour = through-cut paths"));
    assert!(print_guide.contains("Crease = fold/scoring lines (not through-cuts)"));
    assert!(print_guide.contains("the PDF alone is not a cutting file"));
    assert!(print_guide.contains(&format!(
        "{:.3} mm × {:.3} mm",
        physical_mm("width"),
        physical_mm("height")
    )));
    assert!(print_guide.contains("Raster finishing plates: 36 PPI"));
    assert!(print_guide.contains("scale (actual size); turn off fit-to-page"));
    assert!(print_guide.contains("Flat artwork media size:"));
    assert!(print_guide.contains("PDF/X"));
    assert!(print_guide.contains("not a manufacturing-certified package"));
    assert!(flat_svg.contains("data-direction=\"valley\""));
    assert!(flat_svg.contains("stroke=\"#2563eb\""));
    assert!(flat_svg.contains("data-direction=\"mountain\""));
    assert!(flat_svg.contains("stroke=\"#7c3aed\""));
    assert!(flat_svg.contains("stroke-dasharray=\"1.5 1\""));
    let cutting_map = std::fs::read_to_string(bundle.join("CUTTING-MAP.svg")).unwrap();
    assert!(cutting_map.contains("id=\"cutting-map-legend\""));
    assert!(cutting_map.contains("CUT · through-cut (red)"));
    assert!(cutting_map.contains("CREASE · fold/score, not a cut (blue)"));
    assert!(cutting_map.contains("Valley fold · blue solid"));
    assert!(cutting_map.contains("Mountain fold · violet dashed"));
    assert!(cutting_map.contains("data-direction=\"mountain\""));
    assert!(cutting_map.contains("stroke-dasharray=\"1.5 1\""));
    assert!(cutting_map.contains("PERFORATION · tear line (purple)"));
    assert!(cutting_map.contains("BLEED · artwork extension beyond trim (orange)"));
    assert!(cutting_map.contains("SAFE ZONE · keep important artwork inside (green)"));
    assert!(cutting_map.contains("GLUE FLAP · adhesive area; keep critical art clear (cyan)"));
    assert!(cutting_map.contains("data-reference-only=\"true\""));

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let plates = manifest["files"]["vector_spot_plates"]["report"]["plates"]
        .as_array()
        .unwrap();
    for (ink, role) in [
        ("CutContour", "cut"),
        ("Crease", "crease"),
        ("Perforation", "perforation"),
    ] {
        let plate = plates
            .iter()
            .find(|plate| plate["ink"] == ink)
            .unwrap_or_else(|| panic!("missing {ink} vector plate: {plates:?}"));
        let svg = std::fs::read_to_string(
            bundle
                .join("vector-spot-plates")
                .join(plate["path"].as_str().unwrap()),
        )
        .unwrap();
        assert!(svg.contains(&format!("data-spot-ink=\"{ink}\"")));
        assert!(svg.contains(&format!("data-role=\"{role}\"")));
    }
    let flat = std::fs::read_to_string(bundle.join("flat.svg")).unwrap();
    assert!(flat.contains("data-role=\"crease\""));
    assert_eq!(std::fs::read(&document).unwrap(), original);

    let verify = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let verified: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(verified["status"], "verified");
    assert!(verified["artifacts_verified"].as_u64().unwrap() >= 6);
    assert!(verified["vector_plates_verified"].as_u64().unwrap() >= 3);
    assert!(verified["production_roles"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("cut")));
    assert!(verified["production_roles"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("crease")));
    assert!(verified["production_roles"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("perforation")));

    // Rehashing a misleading review map must not turn it into a valid print
    // handoff; verification checks its production guidance, not hashes alone.
    let cutting_map_path = bundle.join("CUTTING-MAP.svg");
    let cutting_map = std::fs::read_to_string(&cutting_map_path).unwrap();
    let misleading_map = cutting_map.replace("CUT · through-cut (red)", "CUT · fold only (red)");
    assert_ne!(misleading_map, cutting_map);
    std::fs::write(&cutting_map_path, &misleading_map).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    manifest["files"]["cutting_map"]["bytes"] = serde_json::json!(misleading_map.len());
    manifest["files"]["cutting_map"]["sha256"] =
        serde_json::json!(pentool::resource::sha256(misleading_map.as_bytes()));
    std::fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let verify_misleading = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify_misleading.status.success());
    assert!(String::from_utf8_lossy(&verify_misleading.stderr)
        .contains("changes required production guidance"));

    // The visual map's actual vector geometry must remain the same as flat.svg.
    std::fs::write(&cutting_map_path, &cutting_map).unwrap();
    let cut_marker = cutting_map.find("data-role=\"cut\"").unwrap();
    let cut_path_start = cutting_map[..cut_marker].rfind("<path ").unwrap();
    let d_value_start = cut_path_start + cutting_map[cut_path_start..].find(" d=\"").unwrap() + 4;
    let d_value_end = d_value_start + cutting_map[d_value_start..].find('"').unwrap();
    let changed_geometry = format!(
        "{}M 0 0 L 1 1{}",
        &cutting_map[..d_value_start],
        &cutting_map[d_value_end..]
    );
    std::fs::write(&cutting_map_path, &changed_geometry).unwrap();
    manifest["files"]["cutting_map"]["bytes"] = serde_json::json!(changed_geometry.len());
    manifest["files"]["cutting_map"]["sha256"] =
        serde_json::json!(pentool::resource::sha256(changed_geometry.as_bytes()));
    std::fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let verify_moved_cut = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify_moved_cut.status.success());
    assert!(String::from_utf8_lossy(&verify_moved_cut.stderr)
        .contains("production geometry does not match"));

    // The machine-facing plate must also retain its declared operation when
    // its digest and manifest entry are both recomputed.
    std::fs::write(&cutting_map_path, &cutting_map).unwrap();
    manifest["files"]["cutting_map"]["bytes"] = serde_json::json!(cutting_map.len());
    manifest["files"]["cutting_map"]["sha256"] =
        serde_json::json!(pentool::resource::sha256(cutting_map.as_bytes()));
    let cut_plate = plates
        .iter()
        .find(|plate| plate["ink"] == "CutContour")
        .unwrap();
    let cut_plate_path = bundle
        .join("vector-spot-plates")
        .join(cut_plate["path"].as_str().unwrap());
    let cut_plate_svg = std::fs::read_to_string(&cut_plate_path).unwrap();
    let misleading_plate = cut_plate_svg.replace("data-role=\"cut\"", "data-role=\"crease\"");
    assert_ne!(misleading_plate, cut_plate_svg);
    std::fs::write(&cut_plate_path, &misleading_plate).unwrap();
    let plate_metadata = manifest["files"]["vector_spot_plates"]["report"]["plates"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|plate| plate["ink"] == "CutContour")
        .unwrap();
    plate_metadata["bytes"] = serde_json::json!(misleading_plate.len());
    plate_metadata["sha256"] =
        serde_json::json!(pentool::resource::sha256(misleading_plate.as_bytes()));
    std::fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let verify_misleading_plate = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify_misleading_plate.status.success());
    assert!(String::from_utf8_lossy(&verify_misleading_plate.stderr)
        .contains("canonical spot ink `CutContour` is paired with `crease`"));

    // Updating the plate checksum and manifest must not legitimize moved
    // cutter geometry. Verification compares actual vector paths to flat.svg.
    std::fs::write(&cut_plate_path, &cut_plate_svg).unwrap();
    let (plate_prefix, plate_rest) = cut_plate_svg.split_once(" d=\"").unwrap();
    let (plate_d, plate_suffix) = plate_rest.split_once('"').unwrap();
    let shifted_plate = format!("{plate_prefix} d=\"M 0 0 L 1 1 {plate_d}\"{plate_suffix}");
    std::fs::write(&cut_plate_path, &shifted_plate).unwrap();
    let plate_metadata = manifest["files"]["vector_spot_plates"]["report"]["plates"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|plate| plate["ink"] == "CutContour")
        .unwrap();
    plate_metadata["bytes"] = serde_json::json!(shifted_plate.len());
    plate_metadata["sha256"] =
        serde_json::json!(pentool::resource::sha256(shifted_plate.as_bytes()));
    std::fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let verify_moved_plate = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify_moved_plate.status.success());
    assert!(String::from_utf8_lossy(&verify_moved_plate.stderr)
        .contains("production geometry does not match the flat dieline role `cut`"));

    // A cutter plate with the right paths but the wrong physical scale is
    // equally unsafe; recomputed hashes must not make it pass verification.
    std::fs::write(&cut_plate_path, &cut_plate_svg).unwrap();
    let old_width = cut_plate_svg
        .split_once(" width=\"")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    let old_width_mm = old_width
        .strip_suffix("mm")
        .unwrap()
        .parse::<f64>()
        .unwrap();
    let wrong_width = format!("{}mm", old_width_mm / 2.0);
    let wrongly_scaled_plate = cut_plate_svg.replacen(
        &format!("width=\"{old_width}\""),
        &format!("width=\"{wrong_width}\""),
        1,
    );
    std::fs::write(&cut_plate_path, &wrongly_scaled_plate).unwrap();
    let plate_metadata = manifest["files"]["vector_spot_plates"]["report"]["plates"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|plate| plate["ink"] == "CutContour")
        .unwrap();
    plate_metadata["bytes"] = serde_json::json!(wrongly_scaled_plate.len());
    plate_metadata["sha256"] =
        serde_json::json!(pentool::resource::sha256(wrongly_scaled_plate.as_bytes()));
    std::fs::write(
        bundle.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let verify_wrong_scale = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "verify-bundle", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!verify_wrong_scale.status.success());
    assert!(String::from_utf8_lossy(&verify_wrong_scale.stderr)
        .contains("physical page dimensions do not match the flat dieline"));
    assert_eq!(std::fs::read(&document).unwrap(), original);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn scene3d_surfaces_compose_in_document_stack_order_with_vector_art() {
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(128, 128)).unwrap();
    let mut base = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    base["product_forms"]["carton"] =
        pentool::scene3d::form::starter_preset("shipping-carton").unwrap();
    let page = base["pages"][0]["id"].as_str().unwrap().to_owned();
    let rect = serde_json::json!({
        "kind":"rect", "id":"green-underlay", "x":0, "y":0,
        "width":128, "height":128,
        "style":{"fill":{"fallback":"#00ff00"}}
    });
    let mockup = serde_json::json!({
        "kind":"scene3d", "id":"carton-render", "x":0, "y":0,
        "width":128, "height":128, "product_form":"carton"
    });
    let render = |nodes: Value| {
        let mut document = base.clone();
        document["pages"][0]["layers"][0]["nodes"] = nodes;
        pentool::scene::validate(&document).unwrap();
        pentool::composite::render(&document, std::path::Path::new("."), Some(&page), 1.0).unwrap()
    };
    let vector_on_top = render(serde_json::json!([mockup.clone(), rect.clone()]));
    let scene3d_on_top = render(serde_json::json!([rect, mockup]));
    assert_eq!(vector_on_top.get_pixel(64, 64).0, [0, 255, 0, 255]);
    assert_ne!(
        vector_on_top.get_pixel(64, 64),
        scene3d_on_top.get_pixel(64, 64),
        "the 3D object must participate in the same deterministic z-order as vector art"
    );

    let multiply_mockup = serde_json::json!({
        "kind":"scene3d", "id":"carton-multiply", "x":0, "y":0,
        "width":128, "height":128, "product_form":"carton",
        "blend_mode":"multiply"
    });
    let multiplied = render(serde_json::json!([rect, multiply_mockup]));
    assert_eq!(
        multiplied.get_pixel(64, 64).0,
        [0, scene3d_on_top.get_pixel(64, 64).0[1], 0, 255],
        "multiply on a scene3d surface must blend against the green underlay, not silently use normal"
    );
}

#[test]
fn scene3d_surfaces_use_the_declared_managed_working_space_and_intent() {
    use pentool::color::{
        icc::Intent,
        value::{Destination, Evaluator, NativeColor, ProfileSet},
    };
    use std::path::Path;

    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(128, 128)).unwrap();
    let mut document = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    document["product_forms"]["working-space-carton"] =
        pentool::scene3d::form::starter_preset("shipping-carton").unwrap();
    document["pages"][0]["layers"][0]["nodes"] = serde_json::json!([{
        "kind":"scene3d", "id":"working-space-preview", "x":0, "y":0,
        "width":128, "height":128, "product_form":"working-space-carton"
    }]);
    let page_id = document["pages"][0]["id"].as_str().unwrap().to_owned();
    pentool::scene::validate(&document).unwrap();
    let reference = pentool::composite::render(
        &document,
        Path::new("target/scene3d-working-space.pen"),
        Some(&page_id),
        1.0,
    )
    .unwrap();

    document["color_management"]["roles"] = serde_json::json!({"working":{
        "destination":{"space":"display-p3:linear"},
        "intent":"relative-colorimetric"
    }});
    let working = pentool::composite::render_working(
        &document,
        Path::new("target/scene3d-working-space.pen"),
        Some(&page_id),
        1.0,
        false,
    )
    .unwrap();
    assert_eq!(working.working_space.space(), "display-p3:linear");
    assert_eq!(working.intent, Intent::RelativeColorimetric);

    let pixel = reference.get_pixel(64, 64).0;
    assert!(pixel[3] > 0, "sample point should hit the rendered 3D form");
    let set = ProfileSet::load(
        &document,
        Path::new("target/scene3d-working-space.pen"),
        std::iter::empty(),
    )
    .unwrap();
    let mut evaluator = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let expected = evaluator
        .working(
            &NativeColor::parse(&serde_json::json!({
                "space":"srgb",
                "channels":[pixel[0] as f64 / 255.0,pixel[1] as f64 / 255.0,pixel[2] as f64 / 255.0],
                "alpha":pixel[3] as f64 / 255.0
            }))
            .unwrap(),
            &Destination::parse(&serde_json::json!({"space":"display-p3:linear"})).unwrap(),
        )
        .unwrap();
    for (actual, expected) in working.surface.pixels()[64 * 128 + 64]
        .iter()
        .zip(expected.rgba)
    {
        assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
    }
}

#[test]
fn product_form_mockup_is_included_in_process_color_separations() {
    use pentool::color::spot::{Ink, Library, Provenance};

    let dir = scratch();
    let document_path = dir.join("mockup.pen");
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = serde_json::json!({
        "width": 128,
        "height": 128,
        "background": "#ffffffff"
    });
    pentool::color::catalog::import(
        &mut raw,
        &document_path,
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
    let mut raw = pentool::scene3d::node::migrate_to_v11(raw).unwrap();
    raw["product_forms"]["print-card"] =
        pentool::scene3d::form::starter_preset("product-card").unwrap();
    let page = &mut raw["pages"][0];
    page["layers"][0]["nodes"] = serde_json::json!([{
        "kind":"scene3d", "id":"print-mockup", "product_form":"print-card",
        "x":0, "y":0, "width":128, "height":128
    }]);
    page["production"] = serde_json::json!({
        "units_per_inch": 72,
        "trim": {"x": 0, "y": 0, "width": 128, "height": 128},
        "bleed": 0
    });
    pentool::scene::validate(&raw).unwrap();

    let separated = pentool::color::separation::separate(
        &raw,
        &document_path,
        Some(raw["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap();
    assert_eq!((separated.width, separated.height), (128, 128));
    assert_eq!(separated.json()["flattened_scene3d_objects"], 1);
    assert!(separated.json()["warnings"][0]
        .as_str()
        .unwrap()
        .contains("spot inks"));
    assert!(
        ["C", "M", "Y", "K"].iter().any(|id| {
            separated
                .plates
                .iter()
                .find(|plate| plate.id == *id)
                .is_some_and(|plate| plate.samples.iter().any(|sample| *sample > 0.01))
        }),
        "the 3D product form should contribute visible process-color pixels"
    );
    let preflight = pentool::color::preflight::run(
        &raw,
        &document_path,
        Some(raw["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::preflight::Options {
            bleed_scale: None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        preflight["issues"].as_array().unwrap().iter().any(|issue| {
            issue["code"] == "scene3d-print-flattened"
                && issue["node"] == "print-mockup"
                && issue["severity"] == "warning"
        }),
        "print preflight should identify the 3D mockup's lost spot/vector plate semantics: {preflight}"
    );

    // Plate separation intentionally refuses partial alpha and non-normal
    // blending until those interactions have a precise ink/knockout model.
    let mut translucent = raw.clone();
    translucent["pages"][0]["layers"][0]["nodes"][0]["opacity"] = serde_json::json!(0.5);
    let error = pentool::color::separation::separate(
        &translucent,
        &document_path,
        Some(translucent["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("partial-alpha interactions"), "{error}");

    let mut blended = raw.clone();
    blended["pages"][0]["layers"][0]["nodes"][0]["blend_mode"] = serde_json::json!("multiply");
    let error = pentool::color::separation::separate(
        &blended,
        &document_path,
        Some(blended["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("only normal blending separates"),
        "non-normal 3D blend must be refused before plate generation: {error}"
    );

    let mut unsupported_overprint = raw.clone();
    unsupported_overprint["pages"][0]["layers"][0]["nodes"][0]["style"] =
        serde_json::json!({"overprint":{"fill":true}});
    let error = pentool::color::separation::separate(
        &unsupported_overprint,
        &document_path,
        Some(unsupported_overprint["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("cannot preserve vector overprint semantics"),
        "{error}"
    );

    let mut inks = Library::default();
    inks.insert(Ink {
        id: "mockup-spot".into(),
        name: "Mockup Spot".into(),
        aliases: vec![],
        alternate: serde_json::json!({"space":"srgb","channels":[0.8,0.2,0.1]}),
        provenance: Provenance {
            source: "v0.16 separation regression".into(),
            publisher: None,
            revision: None,
            license: None,
        },
        redistribution: "allowed".into(),
    })
    .unwrap();
    let mut unsupported_style_spot = raw.clone();
    unsupported_style_spot["color_management"]["inks"] = serde_json::to_value(inks).unwrap();
    unsupported_style_spot["pages"][0]["layers"][0]["nodes"][0]["style"] = serde_json::json!({"fill":{"spot":{"ink":"mockup-spot","tint":1.0},"fallback":{"space":"srgb","channels":[0.8,0.2,0.1]}}});
    let error = pentool::color::separation::separate(
        &unsupported_style_spot,
        &document_path,
        Some(unsupported_style_spot["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("declares spot ink in style"),
        "a flattened 3D node cannot silently interpret a style-level spot as a process plate: {error}"
    );

    let mut unsupported_trap = raw.clone();
    unsupported_trap["pages"][0]["layers"][0]["nodes"][0]["style"] =
        serde_json::json!({"trap":{"width":0.1}});
    let error = pentool::color::separation::separate(
        &unsupported_trap,
        &document_path,
        Some(unsupported_trap["pages"][0]["id"].as_str().unwrap()),
        &pentool::color::separation::Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("cannot be manually trapped"), "{error}");

    let print_options = pentool::color::pdfx::Options {
        ppi: 72.0,
        created: Some("2026-10-15T00:00:00Z".into()),
        title: Some("3D product print regression".into()),
        ..Default::default()
    };
    let pdf_with_mockup = pentool::color::pdfx::build(
        &raw,
        &document_path,
        Some(raw["pages"][0]["id"].as_str().unwrap()),
        &print_options,
    )
    .unwrap();
    let mut without_mockup = raw.clone();
    without_mockup["pages"][0]["layers"][0]["nodes"] = serde_json::json!([]);
    let pdf_without_mockup = pentool::color::pdfx::build(
        &without_mockup,
        &document_path,
        Some(without_mockup["pages"][0]["id"].as_str().unwrap()),
        &print_options,
    )
    .unwrap();
    assert_ne!(
        pdf_with_mockup.bytes, pdf_without_mockup.bytes,
        "PDF/X output should contain the 3D mockup raster"
    );
    pentool::color::pdfx::verify_structure(&pdf_with_mockup.bytes).unwrap();

    let mut translucent_mockup = raw.clone();
    translucent_mockup["pages"][0]["layers"][0]["nodes"][0]["opacity"] = serde_json::json!(0.5);
    let translucent_pdf = pentool::color::pdfx::build(
        &translucent_mockup,
        &document_path,
        Some(translucent_mockup["pages"][0]["id"].as_str().unwrap()),
        &print_options,
    )
    .unwrap();
    assert_ne!(
        pdf_with_mockup.bytes, translucent_pdf.bytes,
        "PDF/X flattening should preserve scene3d object opacity"
    );
    pentool::color::pdfx::verify_structure(&translucent_pdf.bytes).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stand_up_pouch_cli_is_transactional_renderable_and_listed() {
    let dir = scratch();
    let document = dir.join("pouch.pen");
    std::fs::write(
        &document,
        include_bytes!("../docs/fixtures/v11-product-packaging.pen"),
    )
    .unwrap();
    let path = document.to_str().unwrap();
    let before = std::fs::read(&document).unwrap();
    let dry_run = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            path,
            "--dry-run",
            "pouch",
            "--dieline",
            "pouch-1",
            "--node",
            "pouch-preview",
            "--layer",
            "mockups",
            "--page",
            "products",
            "--front-width-mm",
            "120",
            "--gusset-mm",
            "35",
            "--height-mm",
            "180",
        ])
        .output()
        .unwrap();
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);

    let added = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            path,
            "pouch",
            "--dieline",
            "pouch-1",
            "--node",
            "pouch-preview",
            "--layer",
            "mockups",
            "--page",
            "products",
            "--front-width-mm",
            "120",
            "--gusset-mm",
            "35",
            "--height-mm",
            "180",
        ])
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&document).unwrap()).unwrap();
    let pouch = &saved["dielines"]["pouch-1"];
    assert_eq!(pouch["template"], "pouch");
    assert_eq!(pouch["panels"].as_array().unwrap().len(), 5);

    let render = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["scene3d", path, "render-report", "pouch-preview"])
        .output()
        .unwrap();
    assert!(
        render.status.success(),
        "{}",
        String::from_utf8_lossy(&render.stderr)
    );
    let report: Value = serde_json::from_slice(&render.stdout).unwrap();
    assert!(report["render"]["output"]["width_px"].as_u64().unwrap() > 0);

    let bottle_label = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "pack",
            path,
            "bottle-label",
            "--dieline",
            "bottle-label-1",
            "--node",
            "bottle-label-preview",
            "--layer",
            "mockups",
            "--page",
            "products",
            "--radius-mm",
            "30",
            "--height-mm",
            "90",
            "--overlap-mm",
            "3",
        ])
        .output()
        .unwrap();
    assert!(
        bottle_label.status.success(),
        "{}",
        String::from_utf8_lossy(&bottle_label.stderr)
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&document).unwrap()).unwrap();
    let wrap = &saved["dielines"]["bottle-label-1"];
    assert_eq!(wrap["template"], "bottle-label");
    assert_eq!(wrap["panels"][0]["id"], "bottle-label");
    assert_eq!(wrap["wrap"]["overlap_mm"], 3.0);
    let label_render = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["scene3d", path, "render-report", "bottle-label-preview"])
        .output()
        .unwrap();
    assert!(
        label_render.status.success(),
        "{}",
        String::from_utf8_lossy(&label_render.stderr)
    );
    let label_report: Value = serde_json::from_slice(&label_render.stdout).unwrap();
    assert_eq!(label_report["render"]["output"]["format"], "png");

    let templates = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["pack", "templates"])
        .output()
        .unwrap();
    assert!(templates.status.success());
    let catalog: Value = serde_json::from_slice(&templates.stdout).unwrap();
    assert!(catalog["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == "pouch" && item["panels"] == 5));
    assert!(catalog["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == "bottle-label" && item["panels"] == 1));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_builtin_template_preflights_without_panel_intersections_at_each_fold() {
    use pentool::scene3d::pack;
    let fixture = include_bytes!("../docs/fixtures/v11-product-packaging.pen");
    for template in ["straight-tuck", "reverse-tuck", "sleeve", "mailer", "pouch"] {
        let mut document: Value = serde_json::from_slice(fixture).unwrap();
        let (id, node) = ("built-in", "built-in-preview");
        let placement = [600.0, 100.0, 400.0, 600.0];
        let (layer, page) = ("mockups", Some("products"));
        match template {
            "sleeve" => pack::add_sleeve(
                &mut document,
                id,
                node,
                layer,
                page,
                120.0,
                70.0,
                180.0,
                0.5,
                3.0,
                placement,
            ),
            "pouch" => pack::add_pouch(
                &mut document,
                id,
                node,
                layer,
                page,
                120.0,
                35.0,
                180.0,
                3.0,
                placement,
            ),
            "mailer" => pack::add_tuck_carton(
                &mut document,
                id,
                node,
                layer,
                page,
                template,
                180.0,
                120.0,
                35.0,
                0.6,
                3.0,
                18.0,
                placement,
            ),
            _ => pack::add_tuck_carton(
                &mut document,
                id,
                node,
                layer,
                page,
                template,
                120.0,
                70.0,
                180.0,
                0.5,
                3.0,
                15.0,
                placement,
            ),
        }
        .unwrap();
        for fold in [0.0, 0.5, 1.0] {
            document["dielines"][id]["fold_state"] = serde_json::json!(fold);
            let report =
                pack::preflight(&document, std::path::Path::new("built-in.pen"), id, 300.0)
                    .unwrap();
            let collisions = &report["fold_collision_report"];
            // Documented, precisely pinned exceptions (everything else must be
            // clear):
            // - straight-tuck mid-fold: with one shared fold amount the side
            //   dust flaps and the lids sweep through each other in transit.
            //   Real assembly folds flaps before lids; the closed state is
            //   layered and clear.
            // - mailer once folded: the lid overlaps the side glue joint; the
            //   mailer net is outside the dust-flap fix and remains open.
            let known: Option<fn(&[&str]) -> bool> = match (template, fold > 0.0) {
                ("straight-tuck", _) if fold == 0.5 => Some(|panels| {
                    panels.iter().any(|id| id.contains("dust"))
                        && panels.iter().any(|id| id.ends_with("-lid"))
                }),
                ("mailer", true) => {
                    Some(|panels| panels.contains(&"lid") && panels.contains(&"glue-joint"))
                }
                _ => None,
            };
            if let Some(known) = known {
                for collision in collisions["collisions"].as_array().unwrap() {
                    let panels: Vec<&str> = collision["panels"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|panel| panel.as_str().unwrap())
                        .collect();
                    assert!(
                        known(&panels),
                        "{template} at fold {fold}: unexpected pair in {collisions}"
                    );
                }
                continue;
            }
            assert_eq!(
                collisions["collision_count"], 0,
                "{template} at fold {fold}: {collisions}"
            );
            assert_eq!(
                collisions["status"], "checked-clear",
                "{template} at fold {fold}"
            );
        }
    }
}
