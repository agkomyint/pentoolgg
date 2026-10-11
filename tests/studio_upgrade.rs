//! Studio's blank document is upgraded with the same migrations as `pentool migrate`.
use serde_json::json;

fn studio_blank() -> serde_json::Value {
    json!({
        "format": "pentool",
        "version": 4,
        "name": "Untitled",
        "pages": [{
            "id": "page-1",
            "name": "Page 1",
            "canvas": {"width": 1200, "height": 800, "background": "#ffffff"},
            "layers": [{"id": "layer-1", "name": "Layer 1", "visible": true, "locked": false, "nodes": []}]
        }],
        "styles": {},
        "components": [],
        "fonts": []
    })
}

#[test]
fn blank_studio_document_migrates_to_the_latest_format() {
    let migrated = pentool::color::catalog::migrate(studio_blank()).unwrap();
    assert_eq!(migrated["version"], pentool::color::VERSION);
    assert_eq!(migrated["pages"][0]["id"], "page-1");
    assert_eq!(migrated["pages"][0]["layers"][0]["id"], "layer-1");
}

#[test]
fn each_step_of_the_upgrade_ladder_reaches_its_version() {
    let v5 = pentool::scene::migrate_to_v5(studio_blank()).unwrap();
    assert_eq!(v5["version"], 5);
    let v6 = pentool::composite::migrate(v5).unwrap();
    assert_eq!(v6["version"], 6);
    let v7 = pentool::photo::catalog::migrate(v6).unwrap();
    assert_eq!(v7["version"], 7);
}
