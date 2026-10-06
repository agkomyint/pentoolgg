use pentool::{
    agent::{self, ObjectAction, ObjectKind},
    document::{Document, Path, StrokeCap, StrokeJoin},
};

fn doc() -> Document {
    let mut d = Document::new(400, 300);
    d.layers[0].id = "art".into();
    d.layers[0].name = "Artwork".into();
    d.layers[0].paths.push(Path {
        id: "composer".into(),
        d: "M 10 10 L 100 10 L 100 60 Z".into(),
        stroke: "none".into(),
        stroke_width: 0.0,
        stroke_linecap: StrokeCap::Butt,
        stroke_linejoin: StrokeJoin::Miter,
        stroke_miterlimit: 4.0,
        fill: "#111111".into(),
        closed: true,
    });
    let mut second = d.layers[0].clone();
    second.id = "other".into();
    second.name = "Other".into();
    second.paths.clear();
    d.layers.push(second);
    d
}
fn set_fill(id: &str, layer: &str, fill: &str) -> ObjectAction {
    ObjectAction::Set {
        id: id.into(),
        layer: layer.into(),
        d: None,
        stroke: None,
        width: None,
        fill: Some(fill.into()),
        cap: None,
        join: None,
        miter_limit: None,
        content: None,
        x: None,
        y: None,
        font: None,
        size: None,
        weight: None,
        italic: None,
        align: None,
        letter_spacing: None,
        line_height: None,
        blend: None,
        blend_space: None,
        opacity: None,
        content_opacity: None,
        isolation: None,
    }
}

#[test]
fn search_returns_stable_ids_bounds_and_layer_state() {
    let d = doc();
    let v = agent::inspect(&d, Some("compos"), Some(ObjectKind::Path), None).unwrap();
    assert_eq!(v["matches"], 1);
    assert_eq!(v["layers"][0]["objects"][0]["id"], "composer");
    assert!(
        v["layers"][0]["objects"][0]["bounds"]["width"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(v["layers"][0]["locked"], false);
}

#[test]
fn search_pagination_is_compact_and_reports_total_matches() {
    let mut d = doc();
    let mut second = d.layers[0].paths[0].clone();
    second.id = "composer-two".into();
    d.layers[0].paths.push(second);
    let first = agent::inspect_paginated(&d, Some("composer"), None, None, 0, 1).unwrap();
    assert_eq!(first["matches"], 2);
    assert_eq!(first["returned"], 1);
    assert_eq!(first["has_more"], true);
    let second = agent::inspect_paginated(&d, Some("composer"), None, None, 1, 1).unwrap();
    assert_eq!(second["layers"][0]["objects"][0]["id"], "composer-two");
}

#[test]
fn partial_edit_duplicate_move_and_extensions_survive() {
    let mut d = doc();
    let mut raw = serde_json::to_value(&d).unwrap();
    raw["pages"][0]["layers"][0]["paths"][0]["plugin_data"] = serde_json::json!({"keep":true});
    let actions = vec![
        set_fill("composer", "art", "#ff0000"),
        ObjectAction::Rename {
            id: "composer".into(),
            layer: "art".into(),
            new_id: "panel".into(),
        },
        ObjectAction::Duplicate {
            id: "panel".into(),
            layer: "art".into(),
            new_id: "panel-copy".into(),
        },
        ObjectAction::MoveToLayer {
            id: "panel-copy".into(),
            layer: "art".into(),
            target_layer: "other".into(),
        },
    ];
    agent::apply_batch(&mut d, &actions).unwrap();
    let encoded = serde_json::to_value(&d).unwrap();
    assert_eq!(
        encoded["pages"][0]["layers"][0]["paths"][0]["fill"],
        "#ff0000"
    );
    agent::merge_document(&mut raw, &d, &actions).unwrap();
    let layers = &raw["pages"][0]["layers"];
    assert_eq!(layers[0]["paths"][0]["id"], "panel");
    assert_eq!(layers[0]["paths"][0]["fill"], "#ff0000");
    assert_eq!(layers[0]["paths"][0]["plugin_data"]["keep"], true);
    assert_eq!(layers[1]["paths"][0]["plugin_data"]["keep"], true);
}

#[test]
fn failed_batch_is_atomic_and_locks_are_honored() {
    let mut d = doc();
    let before = serde_json::to_value(&d).unwrap();
    let actions = vec![
        set_fill("composer", "art", "red"),
        ObjectAction::Rename {
            id: "missing".into(),
            layer: "art".into(),
            new_id: "x".into(),
        },
    ];
    assert!(agent::apply_batch(&mut d, &actions).is_err());
    assert_eq!(serde_json::to_value(&d).unwrap(), before);
    d.layers[0].locked = true;
    assert!(agent::apply(&mut d, &set_fill("composer", "art", "red")).is_err());
}

#[test]
fn transactional_write_keeps_exact_recovery_snapshot() {
    let root = std::env::temp_dir().join(format!("pentool-agent-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("design.pen");
    std::fs::write(&file, b"old").unwrap();
    let backup = pentool::editing::transactional_write(&file, b"new").unwrap();
    assert_eq!(std::fs::read(file).unwrap(), b"new");
    assert_eq!(std::fs::read(backup).unwrap(), b"old");
    std::fs::remove_dir_all(root).unwrap();
}
