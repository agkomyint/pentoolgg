use pentool::{asset, history, instance, render};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pentool-v064-{}-{}-{name}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn source(version: &str, fill: &str, label: &str) -> Value {
    json!({"format":"pentool","version":3,"name":"Card","fonts":[],"pages":[{"id":"asset","name":"Asset","canvas":{"width":920,"height":400,"background":"none"},"layers":[{"id":"content","name":"Content","visible":true,"locked":false,"paths":[{"id":"body","d":"M10 10 L90 10 L90 90 L10 90 Z","stroke":"none","stroke_width":0,"fill":fill,"closed":true}],"texts":[{"id":"label","content":label,"x":20,"y":55,"font_family":"Atkinson Hyperlegible","font_size":16,"font_weight":400,"italic":false,"fill":"#ffffff","align":"left","letter_spacing":0,"line_height":1.2,"transform":[1,0,0,1,0,0]}]}]}],"asset":{"id":"ui/card","name":"Card","asset_version":version,"entry_page":"asset","bounds":{"x":10,"y":10,"width":80,"height":80},"properties":{"label":{"type":"text","targets":[{"object":"label","property":"content"}]}}}})
}

fn red_foreground_pixels(raw: &Value) -> usize {
    let document: pentool::document::Document = serde_json::from_value(raw.clone()).unwrap();
    let png = render::to_png(&document, 1.0).unwrap();
    tiny_skia::Pixmap::decode_png(&png)
        .unwrap()
        .pixels()
        .iter()
        .filter(|pixel| pixel.red() > 180 && pixel.green() < 120 && pixel.blue() < 120)
        .count()
}

#[test]
fn six_pages_update_atomically_preserving_overrides_and_stack() {
    let document_path = temp("deck.pen");
    let source_path = temp("card-v2.pen");
    let mut document = json!({"format":"pentool","version":3,"name":"Six pages","fonts":[],"pages":[],"instances":[]});
    for index in 1..=6 {
        let instance_id = format!("card-{index}");
        let layer_id = format!("{instance_id}-content");
        let layer = json!({"id":layer_id,"name":"Card","visible":index != 3,"locked":index == 4,"paths":[{"id":format!("{instance_id}-body"),"d":"M10 10 L90 10 L90 90 L10 90 Z","stroke":"none","stroke_width":0,"fill":"#334155","closed":true}],"texts":[{"id":format!("{instance_id}-label"),"content":format!("Page {index}"),"x":20,"y":55,"font_family":"Atkinson Hyperlegible","font_size":16,"font_weight":400,"italic":false,"fill":"#ffffff","align":"left","letter_spacing":0,"line_height":1.2,"transform":[1,0,0,1,0,0]}]});
        document["pages"].as_array_mut().unwrap().push(json!({"id":format!("page-{index}"),"name":format!("Page {index}"),"canvas":{"width":200,"height":140,"background":"#ffffff"},"layers":[{"id":format!("back-{index}"),"name":"Back","visible":true,"locked":false,"paths":[],"texts":[]},layer,{"id":format!("check-{index}"),"name":"Foreground check","visible":true,"locked":true,"paths":[{"id":format!("check-path-{index}"),"d":"M40 55 L55 70 L85 35","stroke":"#ef4444","stroke_width":8,"fill":"none","closed":false}],"texts":[]}]}));
        let ids = vec![layer_id.clone()];
        let accepted =
            instance::materialized_hash(&document, &format!("page-{index}"), &ids).unwrap();
        let base = instance::selected_layers(&document, &format!("page-{index}"), &ids).unwrap();
        instance::attach(
            &mut document,
            instance::InstanceRecord {
                id: instance_id.clone(),
                library: "fixture".into(),
                asset_id: "ui/card".into(),
                asset_version: "1.0.0".into(),
                content_hash: "sha256:old".into(),
                layer_ids: ids,
                page_id: format!("page-{index}"),
                parent_id: None,
                layer_indices: vec![1],
                layer_states: vec![json!({"visible":index != 3,"locked":index == 4})],
                transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                visible: true,
                overrides: serde_json::Map::from_iter([(
                    "label".into(),
                    json!(format!("Page {index}")),
                )]),
                property_definitions: serde_json::Map::from_iter([(
                    "label".into(),
                    json!({"type":"text","targets":[{"object":"label","property":"content"}]}),
                )]),
                child_ids: vec![
                    json!({"id":"body","new_id":format!("{instance_id}-body")}),
                    json!({"id":"label","new_id":format!("{instance_id}-label")}),
                ],
                local_patches: vec![],
                materialized_hash: Some(accepted),
                base_layers: base,
                previous: vec![],
            },
        )
        .unwrap();
    }
    let original_bytes = serde_json::to_vec_pretty(&document).unwrap();
    let original_red_pixels = red_foreground_pixels(&document);
    assert!(original_red_pixels > 0);
    fs::write(&document_path, &original_bytes).unwrap();
    let incoming = source("1.1.0", "#0f766e", "Incoming");
    fs::write(&source_path, serde_json::to_vec_pretty(&incoming).unwrap()).unwrap();
    let preview = instance::update_bulk(
        &document_path,
        &source_path,
        Some("ui/card"),
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        None,
        true,
    )
    .unwrap();
    assert_eq!(preview["counts"]["skipped"], 0, "{preview:#}");
    let result = instance::update_bulk(
        &document_path,
        &source_path,
        Some("ui/card"),
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        None,
        false,
    )
    .unwrap();
    assert_eq!(result["counts"]["skipped"], 0, "{result:#}");
    assert_eq!(result["counts"]["updated"], 6);
    let updated: Value = serde_json::from_slice(&fs::read(&document_path).unwrap()).unwrap();
    assert!(
        red_foreground_pixels(&updated) * 100 >= original_red_pixels * 95,
        "the foreground check must remain visibly above the updated card"
    );
    for (index, page) in updated["pages"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            page["layers"][1]["id"],
            format!("card-{}-content", index + 1)
        );
        assert_eq!(
            page["layers"][1]["texts"][0]["content"],
            format!("Page {}", index + 1)
        );
        assert!(page["layers"][2]["id"]
            .as_str()
            .unwrap()
            .starts_with("check-"));
    }
    let entries = history::list(&document_path).unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    history::undo(&document_path).unwrap();
    assert_eq!(fs::read(&document_path).unwrap(), original_bytes);
    let _ = fs::remove_file(document_path);
    let _ = fs::remove_file(source_path);
}

#[test]
fn tight_asset_fixture_is_not_source_canvas_sized() {
    let raw = source("1.0.0", "#334155", "Label");
    let manifest = asset::manifest(&raw).unwrap().unwrap();
    let bounds = manifest.bounds.unwrap();
    assert_eq!((bounds.width, bounds.height), (80.0, 80.0));
}
