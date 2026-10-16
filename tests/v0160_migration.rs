use serde_json::json;

#[test]
fn v10_to_v11_and_empty_downgrade_round_trip() {
    let v10 = pentool::scene3d::node::migrate(pentool::scene::new_document(64, 64)).unwrap();
    let v11 = pentool::scene3d::node::migrate_to_v11(v10).unwrap();
    assert_eq!(v11["version"], 11);
    assert_eq!(v11["product_forms"], json!({}));
    assert_eq!(v11["dielines"], json!({}));
    pentool::transaction::validate_value(&v11).unwrap();
    let restored = pentool::scene3d::node::to_v10(v11).unwrap();
    assert_eq!(restored["version"], 10);
    pentool::scene::validate(&restored).unwrap();
}

#[test]
fn cli_migration_target_11_is_dry_run_safe_and_explicit() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pentool-v0160-migrate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("design.pen");
    let original = pentool::scene::new_document(64, 64);
    std::fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();
    let run = |dry_run: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pentool"));
        command.args(["migrate", path.to_str().unwrap(), "--target", "11"]);
        if dry_run {
            command.arg("--dry-run");
        }
        command.output().unwrap()
    };
    let dry_run = run(true);
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
            ["version"],
        4
    );
    let committed = run(false);
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
            ["version"],
        11
    );
}

#[test]
fn downgrade_refuses_v11_product_forms_and_surface_graphics() {
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen")).unwrap();
    let form_error = pentool::scene3d::node::to_v10(document.clone())
        .unwrap_err()
        .to_string();
    assert!(form_error.contains("product form"), "{form_error}");

    document["product_forms"] = json!({});
    document["dielines"] = json!({});
    let nodes = &mut document["pages"][1]["layers"][0]["nodes"];
    nodes[0].as_object_mut().unwrap().remove("product_form");
    nodes[0]["model"] =
        json!("sha256:5f32296497f0c937569f9eac6a5ca28cf3e8123206d765f81c5bf3076ca0ee73");
    let graphic_error = pentool::scene3d::node::to_v10(document)
        .unwrap_err()
        .to_string();
    assert!(
        graphic_error.contains("surface graphics"),
        "{graphic_error}"
    );
}

#[test]
fn downgrade_refuses_dielines_and_editable_uv_islands() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../docs/fixtures/v11-product-packaging.pen")).unwrap();

    let mut with_dieline = fixture.clone();
    with_dieline["product_forms"] = json!({});
    with_dieline["pages"][1]["layers"][0]["nodes"][0]["product_form"] = json!(null);
    with_dieline["pages"][1]["layers"][0]["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("product_form");
    with_dieline["pages"][1]["layers"][0]["nodes"][0]["surface_graphics"] = json!([]);
    let error = pentool::scene3d::node::to_v10(with_dieline)
        .unwrap_err()
        .to_string();
    assert!(error.contains("dieline `shirt-carton`"), "{error}");

    let mut with_islands = fixture;
    with_islands["product_forms"] = json!({});
    with_islands["dielines"] = json!({});
    let nodes = &mut with_islands["pages"][1]["layers"][0]["nodes"];
    nodes[0].as_object_mut().unwrap().remove("product_form");
    nodes[0]["model"] =
        json!("sha256:5f32296497f0c937569f9eac6a5ca28cf3e8123206d765f81c5bf3076ca0ee73");
    nodes[0].as_object_mut().unwrap().remove("surface_graphics");
    nodes[1].as_object_mut().unwrap().remove("dieline");
    nodes[1]["model"] =
        json!("sha256:5f32296497f0c937569f9eac6a5ca28cf3e8123206d765f81c5bf3076ca0ee73");
    with_islands["models"]
        ["sha256:5f32296497f0c937569f9eac6a5ca28cf3e8123206d765f81c5bf3076ca0ee73"]["uv"]["0"]
        ["islands"] = json!([
        { "id": "front", "uv_set": 0, "triangles": [0], "transform": { "scale": [1, 1], "offset": [0, 0], "rotate": 0 } }
    ]);
    let error = pentool::scene3d::node::to_v10(with_islands)
        .unwrap_err()
        .to_string();
    assert!(error.contains("editable UV islands"), "{error}");
}

#[test]
fn normative_v11_invalid_cases_are_rejected_without_mutating_the_fixture() {
    let manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../docs/fixtures/v11-product-packaging-invalid-cases.json"
    ))
    .unwrap();
    let base_bytes = include_bytes!("../docs/fixtures/v11-product-packaging.pen");
    let base: serde_json::Value = serde_json::from_slice(base_bytes).unwrap();
    pentool::scene::validate(&base).unwrap();
    let base_snapshot = base.clone();

    for case in manifest["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let expected = case["expected_error"].as_str().unwrap();
        let mut invalid = base.clone();
        for replacement in case["replace"].as_array().unwrap() {
            let pointer = replacement["path"].as_str().unwrap();
            *invalid
                .pointer_mut(pointer)
                .unwrap_or_else(|| panic!("case {id}: missing JSON pointer {pointer}")) =
                replacement["value"].clone();
        }

        let error = format!("{:#}", pentool::scene::validate(&invalid).unwrap_err());
        assert!(
            error.contains(expected),
            "invalid case {id} expected `{expected}`, got `{error}`"
        );
        assert_eq!(
            base, base_snapshot,
            "invalid case {id} mutated its base value"
        );
    }
}
