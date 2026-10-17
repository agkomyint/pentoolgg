use pentool::{interchange, scene, transaction};
use serde_json::json;
use std::{fs, process::Command};

/// Exercise the same JSON persistence boundary used for an ordinary `.pen`
/// file, so import tests also catch properties lost during serialization.
fn save_and_reopen_pen(document: &serde_json::Value) -> serde_json::Value {
    let path = std::env::temp_dir().join(format!(
        "pentool-v017-roundtrip-{}-{}.pen",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
    let reopened = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::remove_file(path).unwrap();
    scene::validate(&reopened).unwrap();
    reopened
}

fn sketch_archive(document: &serde_json::Value, page: &serde_json::Value) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        archive.start_file("document.json", options).unwrap();
        archive
            .write_all(&serde_json::to_vec(document).unwrap())
            .unwrap();
        archive.start_file("pages/roundtrip.json", options).unwrap();
        archive
            .write_all(&serde_json::to_vec(page).unwrap())
            .unwrap();
        archive.finish().unwrap();
    }
    cursor.into_inner()
}

#[test]
fn v12_schema_pins_the_native_gradient_paint_contract() {
    let schema: serde_json::Value =
        serde_json::from_slice(include_bytes!("../docs/pen-format-v12.schema.json")).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["version"]["const"], 12);
    assert!(schema["$defs"]["shapeGradient"]["oneOf"]
        .as_array()
        .is_some_and(|variants| variants.len() == 2));
    assert_eq!(
        schema["$defs"]["gradientStops"]["items"]["properties"]["position"]["minimum"],
        0
    );
    assert_eq!(
        schema["$defs"]["gradientStops"]["items"]["properties"]["position"]["maximum"],
        1
    );
}

#[test]
fn fidelity_report_schema_pins_the_bounded_v1_contract() {
    let schema: serde_json::Value =
        serde_json::from_slice(include_bytes!("../docs/design-interchange-v1.schema.json"))
            .unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);
    assert_eq!(schema["properties"]["items"]["maxItems"], 5000);
    assert_eq!(
        schema["$defs"]["item"]["required"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        schema["$defs"]["item"]["properties"]["status"]["enum"],
        json!(["exact", "approximated", "dropped"])
    );
}

fn svg() -> &'static [u8] {
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40" viewBox="0 0 80 40">
      <g id="marks" transform="translate(2 1)">
        <rect id="box" x="4" y="5" width="20" height="10" fill="#ff0000"/>
        <text id="title">Label</text>
      </g>
    </svg>"##
}

#[test]
fn static_svg_import_returns_scene_and_explicit_fidelity_report() {
    let imported = interchange::import_design("svg", svg(), "marks.svg").unwrap();
    assert!(scene::is_scene_document(&imported.document));
    scene::validate(&imported.document).unwrap();
    assert_eq!(imported.report.format, "svg");
    assert!(imported.report.counts.exact >= 2);
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "title"
            && item.feature == "text"
            && item.status == interchange::FidelityStatus::Exact
    }));
    let nodes = &imported.document["pages"][0]["layers"][0]["nodes"];
    let text = &nodes[0]["children"][0]["children"][1];
    assert_eq!(text["kind"], "text");
    assert_eq!(text["content"], "Label");
}

#[test]
fn svg_conformance_corpus_pins_supported_geometry_paint_text_and_masks() {
    use interchange::FidelityStatus;

    let geometry = interchange::import_design(
        "svg",
        include_bytes!("../docs/fixtures/svg-conformance/geometry-transform.svg"),
        "geometry-transform.svg",
    )
    .unwrap();
    scene::validate(&geometry.document).unwrap();
    assert!(geometry.report.items.iter().any(|item| {
        item.source_id == "translated"
            && item.feature == "group"
            && item.status == FidelityStatus::Exact
    }));
    assert!(geometry.report.items.iter().any(|item| {
        item.source_id == "card"
            && item.feature == "geometry_and_presentation"
            && item.status == FidelityStatus::Exact
    }));
    assert!(geometry
        .report
        .items
        .iter()
        .any(|item| item.source_id == "curve"));

    let gradient = interchange::import_design(
        "svg",
        include_bytes!("../docs/fixtures/svg-conformance/gradient-css.svg"),
        "gradient-css.svg",
    )
    .unwrap();
    scene::validate(&gradient.document).unwrap();
    assert_eq!(gradient.document["version"], 12);
    assert!(serde_json::to_string(&gradient.document)
        .unwrap()
        .contains("\"kind\":\"linear\""));
    assert_eq!(gradient.report.counts.dropped, 0);

    let text = interchange::import_design(
        "svg",
        include_bytes!("../docs/fixtures/svg-conformance/editable-text.svg"),
        "editable-text.svg",
    )
    .unwrap();
    scene::validate(&text.document).unwrap();
    assert!(serde_json::to_string(&text.document)
        .unwrap()
        .contains("\"content\":\"Save\""));
    assert!(text.report.items.iter().any(|item| {
        item.source_id == "label" && item.feature == "text" && item.status == FidelityStatus::Exact
    }));

    let clip_mask = interchange::import_design(
        "svg",
        include_bytes!("../docs/fixtures/svg-conformance/rectangular-clip-mask.svg"),
        "rectangular-clip-mask.svg",
    )
    .unwrap();
    scene::validate(&clip_mask.document).unwrap();
    assert_eq!(clip_mask.document["version"], 12);
    assert!(clip_mask.report.items.iter().any(|item| {
        item.source_id == "clipped"
            && item.feature == "clip_path"
            && item.status == FidelityStatus::Exact
    }));
    assert!(clip_mask.report.items.iter().any(|item| {
        item.source_id == "masked" && item.feature == "mask" && item.status == FidelityStatus::Exact
    }));
    assert!(clip_mask.document["mask_resources"]
        .as_object()
        .is_some_and(|items| !items.is_empty()));
}

#[test]
fn append_namespaces_nodes_and_retains_source_layer_visibility() {
    let mut imported = interchange::import_design("svg", svg(), "marks.svg").unwrap();
    imported.document["pages"][0]["layers"][0]["visible"] = json!(false);
    imported.document["pages"][0]["layers"][0]["locked"] = json!(true);
    let destination = scene::new_document(640, 480);
    let (merged, summary) = interchange::append_imported_document(
        &destination,
        &imported.document,
        None,
        "marks",
        &mut imported.report,
    )
    .unwrap();
    scene::validate(&merged).unwrap();
    assert_eq!(summary["nodes_imported"], 5);
    let imported_layer = &merged["pages"][0]["layers"][1];
    assert_eq!(imported_layer["id"], "marks-import");
    let source_layer_group = &imported_layer["nodes"][0];
    assert_eq!(source_layer_group["visible"], false);
    assert_eq!(source_layer_group["import_source"]["locked"], true);
    assert!(imported.report.items.iter().any(|item| {
        item.feature == "layer_lock" && item.status == interchange::FidelityStatus::Approximated
    }));

    let before = merged.clone();
    assert!(interchange::append_imported_document(
        &merged,
        &imported.document,
        None,
        "marks",
        &mut imported.report,
    )
    .is_err());
    assert_eq!(merged, before);
}

#[test]
fn appending_svg_gradients_migrates_older_destination_to_v12() {
    let source = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><defs><linearGradient id="paint"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient></defs><rect id="shape" width="16" height="16" fill="url(#paint)"/></svg>"##;
    let mut imported = interchange::import_design("svg", source, "gradient.svg").unwrap();
    assert_eq!(imported.document["version"], 12);

    let destination = scene::new_document(64, 64);
    let (merged, _) = interchange::append_imported_document(
        &destination,
        &imported.document,
        None,
        "gradient",
        &mut imported.report,
    )
    .unwrap();

    assert_eq!(merged["version"], 12);
    scene::validate(&merged).unwrap();
    transaction::validate_value(&merged).unwrap();
    let imported_shape = &merged["pages"][0]["layers"][1]["nodes"][0]["children"][0]["children"][0];
    assert_eq!(
        imported_shape["style"]["fill"]["fallback"]["kind"],
        "linear"
    );
}

#[test]
fn cli_svg_export_preserves_imported_gradients_as_vectors() {
    let root = std::env::temp_dir().join(format!(
        "pentool-v017-svg-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let document_path = root.join("gradient.pen");
    let output_path = root.join("gradient.svg");
    let report_path = root.join("report.json");
    let source = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><defs><linearGradient id="paint"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient></defs><rect id="shape" width="16" height="16" fill="url(#paint)"/></svg>"##;
    let imported = interchange::import_design("svg", source, "gradient.svg").unwrap();
    fs::write(
        &document_path,
        serde_json::to_vec(&imported.document).unwrap(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            document_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
            "--report",
            report_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "SVG export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let svg = fs::read_to_string(output_path).unwrap();
    assert!(svg.contains("<linearGradient"), "gradient was flattened");
    assert!(svg.contains("<stop"), "gradient stops were not exported");
    assert!(
        !svg.contains("<image"),
        "vector output contains a raster image"
    );
    let round_trip = interchange::import_design("svg", svg.as_bytes(), "round-trip.svg").unwrap();
    let reopened = save_and_reopen_pen(&round_trip.document);
    assert!(serde_json::to_string(&reopened)
        .unwrap()
        .contains("\"kind\":\"linear\""));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert_eq!(report["counts"]["approximated"], 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn svg_active_content_is_rejected_before_a_document_is_returned() {
    let hostile = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    let result = interchange::import_design("svg", hostile, "hostile.svg");
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("active SVG element"));
}

#[test]
fn design_format_detection_is_unambiguous_for_pen_json() {
    assert_eq!(
        interchange::detect_format(std::path::Path::new("icons.svg")),
        Some("svg")
    );
    assert_eq!(
        interchange::detect_format(std::path::Path::new("file.figma.json")),
        Some("figma")
    );
    assert_eq!(
        interchange::detect_format(std::path::Path::new("document.json")),
        None
    );
    assert_eq!(
        interchange::detect_format(std::path::Path::new("design.fig")),
        Some("figma")
    );
    assert_eq!(
        interchange::detect_format(std::path::Path::new("illustration.ai")),
        Some("pdf")
    );
}

#[test]
fn licensed_figma_v15_binary_imports_bounded_structure_and_reports_drops() {
    let bytes = include_bytes!("../docs/fixtures/figma-weather-now-v15.fig");
    let first = interchange::import_design("figma", bytes, "weather-now.fig").unwrap();
    let second = interchange::import_design("figma", bytes, "weather-now.fig").unwrap();
    assert!(scene::is_scene_document(&first.document));
    scene::validate(&first.document).unwrap();
    assert_eq!(first.report.format, "figma_fig");
    assert_eq!(first.document, second.document);
    assert_eq!(
        serde_json::to_value(&first.report).unwrap(),
        serde_json::to_value(&second.report).unwrap()
    );
    assert_eq!(first.document["pages"].as_array().unwrap().len(), 2);
    assert!(first.report.counts.dropped > 0);
    assert!(first.report.items.iter().any(|item| {
        item.feature == "embedded_blobs" && item.status == interchange::FidelityStatus::Dropped
    }));
}

#[test]
fn cli_autodetects_and_imports_licensed_figma_v15_fixture() {
    let root = std::env::temp_dir().join(format!(
        "pentool-v017-fig-import-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let destination = root.join("document.pen");
    let report = root.join("fig-fidelity.json");
    fs::write(
        &destination,
        serde_json::to_vec(&scene::new_document(1200, 800)).unwrap(),
    )
    .unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/fixtures/figma-weather-now-v15.fig");
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "import",
            destination.to_str().unwrap(),
            fixture.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Figma import failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let imported: serde_json::Value =
        serde_json::from_slice(&fs::read(&destination).unwrap()).unwrap();
    scene::validate(&imported).unwrap();
    assert_eq!(imported["version"], 4);
    let fidelity: serde_json::Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(fidelity["format"], "figma_fig");
    assert!(fidelity["counts"]["dropped"].as_u64().unwrap() > 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn figma_components_and_instances_survive_namespaced_append() {
    let source = json!({
        "document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
            {"id":"component-source","type":"COMPONENT","name":"Button master","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[
                {"id":"master-rect","type":"RECTANGLE","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20}}
            ]},
            {"id":"instance-source","type":"INSTANCE","name":"Button instance","componentId":"component-source","absoluteBoundingBox":{"x":50,"y":10,"width":40,"height":20},"children":[
                {"id":"instance-rect","type":"RECTANGLE","absoluteBoundingBox":{"x":50,"y":10,"width":40,"height":20}}
            ]}
        ]}]}
    });
    let mut imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "components.json",
    )
    .unwrap();
    assert_eq!(imported.document["components"].as_array().unwrap().len(), 1);
    let imported_page_nodes = &imported.document["pages"][0]["layers"][0]["nodes"];
    assert_eq!(imported_page_nodes[1]["kind"], "instance");

    let destination = scene::new_document(200, 100);
    let (merged, _) = interchange::append_imported_document(
        &destination,
        &imported.document,
        None,
        "figma",
        &mut imported.report,
    )
    .unwrap();
    scene::validate(&merged).unwrap();
    let imported_layer = &merged["pages"][0]["layers"][1];
    let instance = &imported_layer["nodes"][0]["children"][1];
    assert_eq!(instance["kind"], "instance");
    assert_eq!(instance["component"], merged["components"][0]["id"]);
    assert_ne!(instance["component"], "component-source");
    assert!(instance["fallback"].is_object());
}

#[test]
fn figma_preserves_node_and_solid_paint_opacity_and_reports_it() {
    let source = json!({
        "document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
            {"id":"shape","type":"RECTANGLE","opacity":0.5,"absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},
             "fills":[{"type":"SOLID","opacity":0.5,"color":{"r":0.2,"g":0.4,"b":0.8,"a":1.0}}],
             "strokes":[{"type":"SOLID","opacity":0.25,"color":{"r":1.0,"g":0.0,"b":0.0,"a":0.5}}],"strokeWeight":2}
        ]}]}
    });
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "opacity.figma.json",
    )
    .unwrap();
    let shape = &imported.document["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(shape["opacity"], json!(0.5));
    assert_eq!(shape["style"]["fill"]["fallback"], "#3366cc80");
    assert_eq!(shape["style"]["stroke"]["fallback"], "#ff000020");
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "shape"
            && item.feature == "node_opacity"
            && item.status == interchange::FidelityStatus::Exact
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "shape"
            && item.feature == "fill_opacity"
            && item.status == interchange::FidelityStatus::Exact
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "shape"
            && item.feature == "stroke_opacity"
            && item.status == interchange::FidelityStatus::Exact
    }));
}

#[test]
fn figma_instance_before_cross_page_master_gets_valid_component_identity() {
    let source = json!({"document":{"type":"DOCUMENT","children":[
        {"id":"first-page","type":"CANVAS","name":"First","children":[
            {"id":"instance","type":"INSTANCE","componentId":"master","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[
                {"id":"instance-child","type":"RECTANGLE","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20}}
            ]}
        ]},
        {"id":"second-page","type":"CANVAS","name":"Second","children":[
            {"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[
                {"id":"master-child","type":"RECTANGLE","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20}}
            ]}
        ]}
    ]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "components.figma.json",
    )
    .unwrap();
    scene::validate(&imported.document).unwrap();
    let instance = &imported.document["pages"][0]["layers"][0]["nodes"][0];
    let component = &imported.document["components"][0];
    assert_eq!(instance["kind"], "instance");
    assert_eq!(instance["component"], component["id"]);
    assert_eq!(instance["source_identity"], component["source_identity"]);
    assert!(!instance["source_identity"].is_null());
}

#[test]
fn figma_auto_layout_keeps_absolute_items_out_of_flow_and_aligns_shape_bounds() {
    let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
        {"id":"frame","type":"FRAME","layoutMode":"HORIZONTAL","itemSpacing":5,"paddingLeft":10,"counterAxisAlignItems":"CENTER","absoluteBoundingBox":{"x":0,"y":0,"width":100,"height":30},"children":[
            {"id":"ellipse","type":"ELLIPSE","absoluteBoundingBox":{"x":40,"y":5,"width":10,"height":10}},
            {"id":"label","type":"TEXT","layoutSizingHorizontal":"HUG","absoluteBoundingBox":{"x":60,"y":0,"width":20,"height":20},"characters":"OK","style":{"fontSize":10}},
            {"id":"absolute","type":"RECTANGLE","layoutPositioning":"ABSOLUTE","absoluteBoundingBox":{"x":77,"y":2,"width":5,"height":5}}
        ]}
    ]}]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "layout.figma.json",
    )
    .unwrap();
    let frame = &imported.document["pages"][0]["layers"][0]["nodes"][0];
    let children = frame["children"].as_array().unwrap();
    assert_eq!(children[0]["cx"], json!(15.0));
    assert_eq!(children[0]["cy"], json!(15.0));
    assert_eq!(children[1]["x"], json!(25.0));
    assert_eq!(children[1]["y"], json!(15.0));
    assert_eq!(
        children[1]["figma_layout_item"]["layoutSizingHorizontal"],
        "HUG"
    );
    assert_eq!(children[2]["x"], json!(77.0));
    assert_eq!(children[2]["y"], json!(2.0));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "frame"
            && item.feature == "auto_layout_constraints"
            && item.status == interchange::FidelityStatus::Approximated
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "absolute"
            && item.feature == "layout_positioning"
            && item.status == interchange::FidelityStatus::Approximated
    }));
}

#[test]
fn figma_wrapped_auto_layout_materializes_rows_and_preserves_absolute_children() {
    let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
        {"id":"frame","type":"FRAME","layoutMode":"HORIZONTAL","layoutWrap":true,"itemSpacing":4,"counterAxisSpacing":6,"counterAxisAlignItems":"CENTER","absoluteBoundingBox":{"x":0,"y":0,"width":54,"height":50},"children":[
            {"id":"a","type":"RECTANGLE","absoluteBoundingBox":{"x":20,"y":20,"width":25,"height":10}},
            {"id":"b","type":"RECTANGLE","absoluteBoundingBox":{"x":20,"y":20,"width":25,"height":15}},
            {"id":"c","type":"RECTANGLE","absoluteBoundingBox":{"x":20,"y":20,"width":25,"height":8}},
            {"id":"absolute","type":"RECTANGLE","layoutPositioning":"ABSOLUTE","absoluteBoundingBox":{"x":41,"y":3,"width":5,"height":5}}
        ]}
    ]}]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "wrapped.figma.json",
    )
    .unwrap();
    let frame = &imported.document["pages"][0]["layers"][0]["nodes"][0];
    let children = frame["children"].as_array().unwrap();
    assert_eq!(children[0]["x"], json!(0.0));
    assert_eq!(children[0]["y"], json!(2.5));
    assert_eq!(children[1]["x"], json!(29.0));
    assert_eq!(children[1]["y"], json!(0.0));
    assert_eq!(children[2]["x"], json!(0.0));
    assert_eq!(children[2]["y"], json!(21.0));
    assert_eq!(children[3]["x"], json!(41.0));
    assert_eq!(children[3]["y"], json!(3.0));
    assert!(imported
        .report
        .items
        .iter()
        .any(|item| item.source_id == "frame" && item.feature == "auto_layout_constraints"));
}

#[test]
fn figma_text_component_properties_apply_to_unique_materialized_fallback_nodes() {
    let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
        {"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[
            {"id":"master-label","type":"TEXT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"characters":"Save","style":{"fontSize":12}}
        ]},
        {"id":"instance","type":"INSTANCE","componentId":"master","absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20},"componentProperties":{
            "Label#text":{"type":"TEXT","value":"Continue"},
            "Disabled":{"type":"BOOLEAN","value":false}
        },"overrides":[{"id":"master-label","overriddenFields":["characters"]}],"children":[
            {"id":"instance-label","type":"TEXT","name":"Label","absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20},"characters":"Save","style":{"fontSize":12}}
        ]}
    ]}]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "properties.figma.json",
    )
    .unwrap();
    let instance = &imported.document["pages"][0]["layers"][0]["nodes"][1];
    assert_eq!(instance["kind"], "instance");
    assert_eq!(
        instance["overrides"]["figma_component_properties"]["Label#text"]["value"],
        "Continue"
    );
    assert_eq!(
        instance["overrides"]["figma_component_properties"]["Disabled"]["value"],
        false
    );
    assert_eq!(
        instance["overrides"]["figma_overrides"][0]["overriddenFields"][0],
        "characters"
    );
    assert_eq!(instance["fallback"]["children"][0]["content"], "Continue");
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "instance"
            && item.feature == "component_override_metadata"
            && item.status == interchange::FidelityStatus::Approximated
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "instance"
            && item.feature == "component_property_text"
            && item.status == interchange::FidelityStatus::Approximated
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "instance"
            && item.feature == "component_instance"
            && item
                .message
                .contains("supported TEXT properties were applied")
    }));
    assert!(imported.report.items.iter().any(|item| {
        item.source_id == "instance"
            && item.feature == "component_property"
            && item.status == interchange::FidelityStatus::Dropped
    }));
}

#[test]
fn figma_import_subset_survives_saving_and_reopening_pen() {
    let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
        {"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[
            {"id":"master-label","type":"TEXT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"characters":"Save","style":{"fontSize":12}}
        ]},
        {"id":"instance","type":"INSTANCE","componentId":"master","opacity":0.75,"absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20},"componentProperties":{
            "Label#text":{"type":"TEXT","value":"Continue"}
        },"children":[
            {"id":"instance-label","type":"TEXT","name":"Label","absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20},"characters":"Save","style":{"fontSize":12}}
        ]}
    ]}]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "roundtrip.figma.json",
    )
    .unwrap();
    let reopened = save_and_reopen_pen(&imported.document);
    let instance = &reopened["pages"][0]["layers"][0]["nodes"][1];
    assert_eq!(reopened["version"], 4);
    assert_eq!(reopened["components"].as_array().unwrap().len(), 1);
    assert_eq!(instance["kind"], "instance");
    assert_eq!(
        instance["overrides"]["figma_component_properties"]["Label#text"]["value"],
        "Continue"
    );
    assert_eq!(instance["fallback"]["children"][0]["content"], "Continue");
    assert_eq!(instance["opacity"], 0.75);
}

#[test]
fn sketch_import_subset_survives_saving_and_reopening_pen() {
    let document = json!({
        "pages":[{"_ref":"pages/roundtrip"}],
        "layerStyles":{"_class":"sharedStyleContainer","objects":[{
            "_class":"sharedStyle","do_objectID":"card-style","name":"Card",
            "value":{"_class":"style",
                "fills":[{"isEnabled":true,"fillType":0,"color":{"red":0.2,"green":0.4,"blue":0.6,"alpha":1},"contextSettings":{"opacity":0.5}}],
                "shadows":[{"isEnabled":true,"offsetX":2,"offsetY":3,"blur":4,"spread":0,"color":{"red":0,"green":0,"blue":0,"alpha":0.3}}],
                "innerShadows":[{"isEnabled":true,"offsetX":-1,"offsetY":1,"blur":2,"spread":0,"color":{"red":1,"green":0,"blue":0,"alpha":0.5}}]
            }
        }]},
        "layerTextStyles":{"_class":"sharedStyleContainer","objects":[{
            "_class":"sharedStyle","do_objectID":"heading-style","name":"Heading",
            "value":{"textStyle":{"paragraphStyle":{"alignment":2},"encodedAttributes":{
                "MSAttributedStringFontAttribute":{"attributes":{"name":"Inter","size":24}}
            }}}
        }]}
    });
    let page = json!({"_class":"page","do_objectID":"page","name":"Round trip","layers":[
        {"_class":"rectangle","do_objectID":"card","sharedStyleID":"card-style","frame":{"x":0,"y":0,"width":100,"height":60}},
        {"_class":"text","do_objectID":"heading","sharedStyleID":"heading-style","frame":{"x":0,"y":70,"width":100,"height":30},"attributedString":{"string":"Title","attributes":[]}}
    ]});
    let bytes = sketch_archive(&document, &page);
    let imported = interchange::import_design("sketch", &bytes, "roundtrip.sketch").unwrap();
    let reopened = save_and_reopen_pen(&imported.document);
    assert_eq!(reopened["version"], 4);
    let nodes = &reopened["pages"][0]["layers"][0]["nodes"];
    assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#33669980");
    assert_eq!(nodes[0]["effects"][0]["kind"], "drop-shadow");
    assert_eq!(nodes[0]["effects"][1]["kind"], "inner-shadow");
    assert_eq!(nodes[0]["effects"][1]["params"]["x"], -1.0);
    assert_eq!(nodes[1]["content"], "Title");
    assert_eq!(nodes[1]["font_family"], "Inter");
    assert_eq!(nodes[1]["font_size"], 24.0);
    assert_eq!(nodes[1]["align"], "center");
}

#[test]
fn figma_instance_with_hidden_or_unsupported_master_becomes_a_reported_group() {
    for master in [
        json!({"id":"master","type":"COMPONENT","visible":false,"absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[]}),
        json!({"id":"vector-parent","type":"VECTOR","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[{"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":0,"y":0,"width":40,"height":20},"children":[]}]}),
    ] {
        let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
            master,
            {"id":"instance","type":"INSTANCE","componentId":"master","absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20},"children":[
                {"id":"instance-child","type":"RECTANGLE","absoluteBoundingBox":{"x":50,"y":0,"width":40,"height":20}}
            ]}
        ]}]}});
        let imported = interchange::import_design(
            "figma",
            &serde_json::to_vec(&source).unwrap(),
            "missing-master.figma.json",
        )
        .unwrap();
        scene::validate(&imported.document).unwrap();
        let nodes = &imported.document["pages"][0]["layers"][0]["nodes"];
        let appearance = nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"].as_str().unwrap_or("").contains("instance"))
            .unwrap();
        assert_eq!(appearance["kind"], "group");
        assert!(imported.report.items.iter().any(|item| {
            item.source_id == "instance"
                && item.feature == "component_reference"
                && item.status == interchange::FidelityStatus::Dropped
        }));
    }
}

#[test]
fn figma_line_and_instance_report_rotation_effects_and_clipping() {
    let source = json!({"document":{"type":"DOCUMENT","children":[{"id":"page","type":"CANVAS","name":"Page","children":[
        {"id":"line","type":"LINE","rotation":15,"absoluteBoundingBox":{"x":0,"y":0,"width":20,"height":2}},
        {"id":"master","type":"COMPONENT","absoluteBoundingBox":{"x":30,"y":0,"width":20,"height":20},"children":[]},
        {"id":"instance","type":"INSTANCE","componentId":"master","rotation":20,"clipsContent":true,"effects":[{"type":"DROP_SHADOW"}],"absoluteBoundingBox":{"x":60,"y":0,"width":20,"height":20},"children":[]}
    ]}]}});
    let imported = interchange::import_design(
        "figma",
        &serde_json::to_vec(&source).unwrap(),
        "features.figma.json",
    )
    .unwrap();
    scene::validate(&imported.document).unwrap();
    for (source_id, feature) in [
        ("line", "rotation"),
        ("instance", "rotation"),
        ("instance", "effects"),
        ("instance", "clip_content"),
    ] {
        assert!(
            imported
                .report
                .items
                .iter()
                .any(|item| { item.source_id == source_id && item.feature == feature }),
            "missing {feature} report for {source_id}"
        );
    }
}

#[test]
fn appending_embedded_raster_assets_upgrades_v4_and_preserves_image_data() {
    fn has_resolvable_image(nodes: &[serde_json::Value], assets: &serde_json::Value) -> bool {
        nodes.iter().any(|node| {
            (node["kind"] == "image"
                && node["asset"]
                    .as_str()
                    .is_some_and(|asset| assets.get(asset).is_some()))
                || node["children"]
                    .as_array()
                    .is_some_and(|children| has_resolvable_image(children, assets))
        })
    }
    let destination: serde_json::Value =
        serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
    let imported: serde_json::Value =
        serde_json::from_str(include_str!("../docs/fixtures/v5-image-embedded.pen")).unwrap();
    let mut report = interchange::FidelityReportBuilder::new("psd", "layers.psd").finish();
    let (merged, _) =
        interchange::append_imported_document(&destination, &imported, None, "psd", &mut report)
            .unwrap();
    scene::validate(&merged).unwrap();
    assert_eq!(merged["version"], 5);
    assert_eq!(
        merged["image_assets"].as_object().unwrap().len(),
        imported["image_assets"].as_object().unwrap().len()
    );
    let imported_layer = &merged["pages"][0]["layers"][1];
    assert!(has_resolvable_image(
        imported_layer["nodes"].as_array().unwrap(),
        &merged["image_assets"]
    ));
}

#[test]
fn cli_v12_image_document_exports_through_image_renderer() {
    let root = std::env::temp_dir().join(format!(
        "pentool-v017-v12-image-export-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source: serde_json::Value =
        serde_json::from_str(include_str!("../docs/fixtures/v5-image-embedded.pen")).unwrap();
    let source = scene::migrate_to_v12(source).unwrap();
    let document_path = root.join("image.pen");
    let output_path = root.join("image.png");
    fs::write(&document_path, serde_json::to_vec(&source).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            document_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "v12 image export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fs::metadata(&output_path).unwrap().len() > 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_all_pages_pdf_report_uses_pdf_fidelity() {
    fn contains_kind(nodes: &[serde_json::Value], kind: &str) -> bool {
        nodes.iter().any(|node| {
            node["kind"] == kind
                || node["children"]
                    .as_array()
                    .is_some_and(|children| contains_kind(children, kind))
        })
    }

    let root = std::env::temp_dir().join(format!(
        "pentool-v017-all-pages-pdf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let document_path = root.join("scene.pen");
    let output_path = root.join("scene.pdf");
    let report_path = root.join("report.json");
    fs::write(
        &document_path,
        include_bytes!("../docs/fixtures/v4-scene.pen"),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            document_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
            "--all-pages",
            "--report",
            report_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "all-pages PDF export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert_eq!(
        report["format"], "pdf",
        "all-pages PDF must use PDF fidelity reporting"
    );
    let round_trip =
        interchange::import_design("pdf", &fs::read(&output_path).unwrap(), "round-trip.pdf")
            .unwrap();
    assert_eq!(round_trip.report.format, "pdf");
    let reopened = save_and_reopen_pen(&round_trip.document);
    assert_eq!(reopened["pages"].as_array().unwrap().len(), 1);
    assert!(reopened["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .is_some_and(|nodes| !nodes.is_empty()));
    assert!(contains_kind(
        reopened["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap(),
        "path"
    ));
    let ai_path = root.join("scene.ai");
    let ai_report_path = root.join("ai-report.json");
    let ai_export = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            document_path.to_str().unwrap(),
            ai_path.to_str().unwrap(),
            "--report",
            ai_report_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        ai_export.status.success(),
        "PDF-compatible Illustrator export failed: {}",
        String::from_utf8_lossy(&ai_export.stderr)
    );
    assert!(fs::read(&ai_path).unwrap().starts_with(b"%PDF-"));
    let ai_report: serde_json::Value =
        serde_json::from_slice(&fs::read(ai_report_path).unwrap()).unwrap();
    assert_eq!(ai_report["format"], "pdf");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_psd_export_import_round_trip_keeps_embedded_raster_assets_in_pen() {
    fn contains_resolvable_image(nodes: &[serde_json::Value], assets: &serde_json::Value) -> bool {
        nodes.iter().any(|node| {
            (node["kind"] == "image"
                && node["asset"]
                    .as_str()
                    .is_some_and(|asset| assets.get(asset).is_some()))
                || node["children"]
                    .as_array()
                    .is_some_and(|children| contains_resolvable_image(children, assets))
        })
    }

    let root = std::env::temp_dir().join(format!(
        "pentool-v017-psd-roundtrip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let output_path = root.join("scene.psd");
    let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args([
            "export",
            "docs/fixtures/v4-scene.pen",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "PSD export failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let imported =
        interchange::import_design("psd", &fs::read(output_path).unwrap(), "scene.psd").unwrap();
    let reopened = save_and_reopen_pen(&imported.document);
    let assets = reopened["image_assets"].as_object().unwrap();
    assert!(!assets.is_empty());
    assert!(contains_resolvable_image(
        reopened["pages"][0]["layers"][0]["nodes"]
            .as_array()
            .unwrap(),
        &reopened["image_assets"]
    ));
    fs::remove_dir_all(root).unwrap();
}
