use pentool::{
    document::{Document, Path, StrokeCap, StrokeJoin},
    import::{compose, ImportOptions},
};

fn source() -> serde_json::Value {
    let mut doc = Document::new(100, 100);
    doc.layers[0].id = "art".into();
    doc.layers[0].paths.push(Path {
        id: "triangle".into(),
        d: "M 0 0 L 20 0 L 0 20 Z".into(),
        stroke: "none".into(),
        stroke_width: 0.0,
        stroke_linecap: StrokeCap::Butt,
        stroke_linejoin: StrokeJoin::Miter,
        stroke_miterlimit: 4.0,
        fill: "red".into(),
        closed: true,
    });
    let mut raw = serde_json::to_value(doc).unwrap();
    raw["pages"][0]["layers"][0]["paths"][0]["plugin_data"] = serde_json::json!({"source":"kept"});
    raw
}

#[test]
fn import_prefixes_transforms_and_preserves_extensions() {
    let destination = serde_json::to_value(Document::new(50, 50)).unwrap();
    let result = compose(
        destination,
        source(),
        &ImportOptions {
            destination_page: None,
            source_page: None,
            prefix: Some("icon".into()),
            x: 100.0,
            y: 40.0,
            scale: 2.0,
            rotation: 0.0,
            expand_canvas: true,
        },
    )
    .unwrap();
    let page = &result.document["pages"][0];
    assert_eq!(page["layers"][1]["id"], "icon-art");
    assert_eq!(page["layers"][1]["paths"][0]["id"], "icon-triangle");
    assert_eq!(
        page["layers"][1]["paths"][0]["plugin_data"]["source"],
        "kept"
    );
    assert!(page["canvas"]["width"].as_u64().unwrap() >= 140);
    let doc: Document = serde_json::from_value(result.document).unwrap();
    doc.validate().unwrap();
}

#[test]
fn no_prefix_collision_is_rejected_without_mutating_input() {
    let destination = source();
    let before = destination.clone();
    assert!(compose(
        destination.clone(),
        source(),
        &ImportOptions {
            destination_page: None,
            source_page: None,
            prefix: None,
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
            expand_canvas: false,
        },
    )
    .is_err());
    assert_eq!(destination, before);
}

#[test]
fn legacy_destination_upgrades_without_losing_original_artwork() {
    let destination = serde_json::json!({
        "format":"pentool","version":1,"name":"Legacy",
        "canvas":{"width":200,"height":100,"background":"#fff"},
        "layers":[{"id":"original","name":"Original","paths":[]}]
    });
    let result = compose(
        destination,
        source(),
        &ImportOptions {
            destination_page: None,
            source_page: None,
            prefix: Some("asset".into()),
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
            expand_canvas: false,
        },
    )
    .unwrap();
    assert_eq!(result.document["version"], 3);
    assert_eq!(result.document["pages"][0]["layers"][0]["id"], "original");
    assert_eq!(result.document["pages"][0]["layers"][1]["id"], "asset-art");
}
