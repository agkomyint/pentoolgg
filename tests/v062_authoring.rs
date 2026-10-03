use pentool::{diff, history, layout, pdf, replace, scene, style, transaction};
use serde_json::{json, Value};
use std::fs;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap()
}

#[test]
fn batch_styles_layout_text_and_components_share_the_v4_scene() {
    let mut value = fixture();
    let operations: Vec<Value> =
        serde_json::from_str(include_str!("../docs/fixtures/v4-batch.json")).unwrap();
    assert_eq!(
        scene::apply_batch(&mut value, None, &operations)
            .unwrap()
            .len(),
        4
    );
    style::apply(
        &mut value,
        style::Operation::Set,
        Some("color.brand"),
        Some("color"),
        Some("#f59e0b"),
        None,
    )
    .unwrap();
    let replacement = replace::apply(
        &mut value,
        &replace::Options {
            page: None,
            all_pages: true,
            layer: None,
            group: Some("batch-card"),
            object_type: None,
            visible: None,
            property: "fill",
            from: "#111827",
            to: "#f59e0b",
        },
    )
    .unwrap();
    assert_eq!(replacement["before_count"], 1);
    scene::promote_component(&mut value, None, "batch-card", "ui.card").unwrap();
    scene::instantiate_component(&mut value, None, "content", "card-2", "missing", 0.0, 0.0)
        .unwrap_err();
    scene::instantiate_component(&mut value, None, "content", "ui.card", "card-2", 280.0, 0.0)
        .unwrap();
    let bounds = scene::node_bounds(&value, None, "batch-card").unwrap();
    assert!(bounds["width"].as_f64().unwrap() > 0.0);
    layout::apply(
        &mut value,
        None,
        layout::Operation::GapH,
        &["batch-card".into(), "card-2".into()],
        Some(24.0),
        None,
        Some("batch-card"),
    )
    .unwrap();
    scene::put_text_box(
        &mut value,
        None,
        "content",
        "body",
        scene::TextBoxInput {
            content: "wrapped text remains deterministic".into(),
            x: 20.0,
            y: 150.0,
            width: Some(120.0),
            height: Some(60.0),
            font: "Atkinson Hyperlegible".into(),
            size: 16.0,
            weight: 400,
            fill: "#111827".into(),
            align: "left".into(),
            vertical_align: "top".into(),
            line_height: 1.2,
            anchor: scene::TextAnchor::Top,
            overflow: scene::TextOverflow::Ellipsis,
        },
    )
    .unwrap();
    scene::validate(&value).unwrap();
    let flattened = scene::flatten_to_v3(&value).unwrap();
    transaction::validate_value(&flattened).unwrap();
    assert!(!diff::structural(&fixture(), &value).is_empty());
}

#[test]
fn transaction_history_undo_redo_and_pdf_are_valid() {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let document = std::env::temp_dir().join(format!("pentool-v062-{unique}.pen"));
    fs::write(&document, include_bytes!("../docs/fixtures/v4-scene.pen")).unwrap();
    let mut changed = fixture();
    scene::translate_node(&mut changed, None, "card", 10.0, 0.0).unwrap();
    let summary = transaction::commit_value(&document, "test", false, None, &changed).unwrap();
    assert!(summary.history.is_some());
    let entries = history::list(&document).unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    history::undo(&document).unwrap();
    assert_eq!(
        fs::read(&document).unwrap(),
        include_bytes!("../docs/fixtures/v4-scene.pen")
    );
    history::redo(&document).unwrap();
    let flattened = scene::flatten_to_v3(&changed).unwrap();
    let doc: pentool::document::Document = serde_json::from_value(flattened).unwrap();
    let output = std::env::temp_dir().join(format!("pentool-v062-{unique}.pdf"));
    pdf::write(&[doc], &output).unwrap();
    let bytes = fs::read(&output).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"));
    assert!(String::from_utf8_lossy(&bytes).contains("/Type /Page"));
}

#[test]
fn malformed_alias_cycles_and_partial_batches_are_rejected() {
    let mut value = fixture();
    style::apply(
        &mut value,
        style::Operation::Set,
        Some("a"),
        Some("color"),
        Some(r#"{"ref":"b"}"#),
        None,
    )
    .unwrap();
    assert!(style::apply(
        &mut value,
        style::Operation::Set,
        Some("b"),
        Some("color"),
        Some(r#"{"ref":"a"}"#),
        None
    )
    .is_err());
    let before = value.clone();
    let operations = json!([{"type":"put-shape","shape":"circle","id":"ok","layer":"content","cx":1,"cy":1,"radius":1},{"type":"unknown","id":"bad"}]);
    assert!(scene::apply_batch(&mut value, None, operations.as_array().unwrap()).is_err());
    assert_eq!(value, before);
}

#[test]
fn feedback_scale_scene_is_one_batch_and_compacts_by_over_25_percent() {
    let mut value = fixture();
    value["pages"][0]["layers"][0]["nodes"] = json!([]);
    let operations = (0..257)
        .map(|index| {
            json!({"type":"put-shape","shape":"rrect","id":format!("object-{index:03}"),"layer":"content","x":((index%16)*24),"y":((index/16)*24),"width":20,"height":20,"radius":4,"fill":"#22D3EE"})
        })
        .collect::<Vec<_>>();
    let changes = scene::apply_batch(&mut value, None, &operations).unwrap();
    assert_eq!(changes.len(), 257);
    scene::validate(&value).unwrap();
    let pretty = serde_json::to_vec_pretty(&value).unwrap();
    let compact = serde_json::to_vec(&value).unwrap();
    assert!(
        compact.len() * 4 <= pretty.len() * 3,
        "compact={} pretty={}",
        compact.len(),
        pretty.len()
    );
}
