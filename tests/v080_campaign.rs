#[path = "support/v080_campaign.rs"]
mod campaign;
use pentool::{composite, diff, history, linked, package, preset, resource, scene, transaction};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v080-campaign-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("sources")).unwrap();
        Self(path)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn photographic_campaign_preserves_sources_history_presets_packages_and_offline_pixels() {
    let ws = Workspace::new();
    for (file, hash) in [
        (
            "earth.jpg",
            "31b3ac8fc83fc84dfc624c9a406ab5ef11134e4e87412dffaeece5cb0dbb14aa",
        ),
        (
            "sunrise.jpg",
            "a50e28b0250f5c2e551b3017a87608dd4db33db6b36d32bda7c40cd677092e62",
        ),
        (
            "horizon.jpg",
            "e63e7dfde437ee354e07f27ccf1555379f950754f01139648587f95cae5b7ab8",
        ),
    ] {
        let bytes = fs::read(format!("examples/v080-campaign/sources/{file}")).unwrap();
        assert_eq!(resource::sha256(&bytes), format!("sha256:{hash}"));
        fs::write(ws.0.join("sources").join(file), bytes).unwrap();
    }
    let document = ws.0.join("campaign.pen");
    let mut raw = campaign::build(&document).unwrap();
    let original = raw.clone();
    let original_assets = raw["image_assets"].clone();
    fs::write(&document, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let before_bytes = fs::read(&document).unwrap();
    let before = composite::render(&raw, &document, Some("poster"), 0.5).unwrap();
    composite::edit(
        &mut raw,
        Some("poster"),
        "set",
        "global-grade",
        &json!({"params":{"amount":-40}}),
    )
    .unwrap();
    let after = composite::render(&raw, &document, Some("poster"), 0.5).unwrap();
    assert_ne!(before, after);
    assert!(diff::structural(&original, &raw)
        .iter()
        .any(|change| change.path.ends_with("/params/amount")));
    transaction::commit_value(
        &document,
        "campaign-grade",
        false,
        Some(&transaction::revision(&before_bytes)),
        &raw,
    )
    .unwrap();
    history::undo(&document).unwrap();
    assert_eq!(fs::read(&document).unwrap(), before_bytes);
    history::redo(&document).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&document).unwrap()).unwrap(),
        raw
    );
    let appearance = preset::capture(&raw, &document, Some("screen"), "screen-design").unwrap();
    assert!(appearance["styles"].get("campaign-shadow").is_some());
    assert!(appearance["styles"].get("campaign-gradient").is_none());
    assert_eq!(appearance["image_assets"], json!({}));
    preset::apply(&mut raw, Some("screen"), "screen-design", &appearance).unwrap();
    scene::validate(&raw).unwrap();
    let compact = serde_json::to_vec(&raw).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&compact).unwrap(), raw);
    assert_eq!(
        raw["image_assets"], original_assets,
        "grading and placement never bake sources"
    );
    assert_eq!(
        raw["mask_resources"].as_object().unwrap().len(),
        1,
        "one immutable mask is reused"
    );
    let analysis =
        pentool::inspect::analyze(&raw, &document, Some("poster"), "page", None, &[[10, 10]])
            .unwrap();
    assert_eq!(analysis["histogram"]["red"].as_array().unwrap().len(), 256);
    let portable = ws.0.join("portable");
    linked::collect(&raw, &document, &portable, false).unwrap();
    let portable_document = portable.join("document.pen");
    let portable_raw: Value =
        serde_json::from_slice(&fs::read(&portable_document).unwrap()).unwrap();
    // An embedded, verified package must preserve all appearances and resources.
    let kit = ws.0.join("kit");
    package::init(&kit, "orbital-study").unwrap();
    let mut embedded = raw.clone();
    for digest in original_assets.as_object().unwrap().keys() {
        linked::edit(&mut embedded, &document, "embed", digest, None, true).unwrap();
    }
    embedded["asset"] = json!({"schema":1,"id":"campaign","name":"Orbital study","asset_version":"0.1.0","kind":"component"});
    fs::write(
        kit.join("assets/campaign.pen"),
        serde_json::to_vec_pretty(&embedded).unwrap(),
    )
    .unwrap();
    let archive = ws.0.join("campaign.penpkg");
    package::pack(&kit, &archive).unwrap();
    package::verify(&archive).unwrap();
    let second_archive = ws.0.join("campaign-again.penpkg");
    package::pack(&kit, &second_archive).unwrap();
    assert_eq!(
        fs::read(&archive).unwrap(),
        fs::read(&second_archive).unwrap()
    );
    let expected = ["poster", "screen"]
        .map(|page| composite::render(&raw, &document, Some(page), 0.5).unwrap());
    fs::rename(ws.0.join("sources"), ws.0.join("disconnected-originals")).unwrap();
    for (page, expected) in ["poster", "screen"].into_iter().zip(expected) {
        assert_eq!(
            composite::render(&portable_raw, &portable_document, Some(page), 0.5).unwrap(),
            expected
        );
        assert_eq!(
            composite::render(&embedded, &kit.join("assets/campaign.pen"), Some(page), 0.5)
                .unwrap(),
            expected
        );
    }
    for (digest, asset) in original_assets.as_object().unwrap() {
        let file = std::path::Path::new(asset["storage"]["path"].as_str().unwrap())
            .file_name()
            .unwrap();
        assert_eq!(
            resource::sha256(&fs::read(ws.0.join("disconnected-originals").join(file)).unwrap()),
            *digest
        );
    }
}
