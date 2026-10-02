use pentool::{asset, document::Document, library, package};
use std::{fs, path::PathBuf};

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "pentool-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn local_asset_index_and_package_round_trip() {
    let root = temp("library-package");
    let source = root.join("source.pen");
    fs::write(
        &source,
        serde_json::to_vec_pretty(&Document::new(320, 200)).unwrap(),
    )
    .unwrap();
    let assets = root.join("library").join("assets");
    let output = assets.join("card.pen");
    asset::create(&asset::CreateOptions {
        input: source,
        output: output.clone(),
        page: None,
        layers: vec![],
        objects: vec![],
        rect: None,
        id: "ui/card".into(),
        name: "Card".into(),
        description: "Reusable card".into(),
        version: "1.0.0".into(),
        kind: "component".into(),
        author: "Pentool".into(),
        license: "MIT".into(),
        tags: vec!["ui".into()],
        category: "layout".into(),
        overwrite: false,
        dry_run: false,
    })
    .unwrap();
    let config = library::project_config(&root);
    let index = library::project_index(&root);
    library::add(&root, &config, "local", &root.join("library")).unwrap();
    library::refresh(&root, &config, &index, None).unwrap();
    let idx = library::load_index(&index).unwrap();
    assert_eq!(idx.assets.len(), 1);
    assert_eq!(
        library::search(
            &idx,
            &library::SearchOptions {
                query: Some("card"),
                library: None,
                tag: None,
                category: None,
                kind: None,
                offset: 0,
                limit: 20
            }
        )["matches"],
        1
    );

    let manifest = serde_json::json!({"schema":1,"name":"open/ui","version":"1.0.0","license":"MIT","assets":{}});
    fs::write(
        root.join("library").join("pentool.package.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let first = root.join("first.penpkg");
    let second = root.join("second.penpkg");
    package::pack(&root.join("library"), &first).unwrap();
    package::pack(&root.join("library"), &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    package::verify(&first).unwrap();
    let registry = root.join("registry");
    package::publish(&first, &registry, false).unwrap();
    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    changed["asset"]["description"] = serde_json::Value::String("changed bytes".into());
    fs::write(&output, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
    let changed_package = root.join("changed.penpkg");
    package::pack(&root.join("library"), &changed_package).unwrap();
    assert!(package::publish(&changed_package, &registry, false)
        .unwrap_err()
        .to_string()
        .contains("immutable"));
    let project = root.join("consumer");
    fs::create_dir_all(&project).unwrap();
    package::install_from_registry(&registry.display().to_string(), "open/ui@1.0.0", &project)
        .unwrap();
    assert_eq!(package::lock_verify(&project).unwrap()["ok"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn package_rejects_escaping_archive_paths() {
    use std::io::Write;
    let root = temp("unsafe-package");
    let path = root.join("unsafe.penpkg");
    let file = fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("../escape.pen", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"bad").unwrap();
    zip.finish().unwrap();
    assert!(package::verify(&path)
        .unwrap_err()
        .to_string()
        .contains("unsafe"));
    fs::remove_dir_all(root).unwrap();
}
