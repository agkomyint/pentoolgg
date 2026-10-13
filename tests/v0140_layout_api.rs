//! v0.14.0 task 17: JSON actions sent by the browser must default like the CLI.
use pentool::{tables, transaction, wrap};
use serde_json::{json, Value};
use std::process::Command;

fn v9_document() -> Value {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("pentool-layout-api-{n}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.pen");
    for args in [
        vec!["new", file.to_str().unwrap(), "--overwrite"],
        vec!["migrate", file.to_str().unwrap(), "--target", "9"],
    ] {
        let status = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(&args)
            .output()
            .unwrap();
        assert!(status.status.success(), "{args:?}");
    }
    let raw = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    raw
}

#[test]
fn table_json_actions_use_the_cli_defaults() {
    let mut raw = v9_document();
    let create =
        serde_json::from_value(json!({"type": "create", "id": "t1", "rows": 2, "columns": 2}))
            .unwrap();
    tables::run(&mut raw, create).unwrap();
    let add = serde_json::from_value(json!({"type": "add-column", "id": "t1", "at": 2})).unwrap();
    tables::run(&mut raw, add).unwrap();
    let table = &raw["layout"]["tables"][0];
    let widths: Vec<f64> = table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["width"].as_f64().unwrap())
        .collect();
    assert_eq!(widths, vec![72.0, 72.0, 72.0]);
    transaction::validate_value(&raw).unwrap();
}

#[test]
fn wrap_json_action_validates_and_round_trips() {
    let mut raw = v9_document();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "rect", "id": "r1", "x": 10, "y": 10, "width": 50, "height": 50}
    ]);
    let set = serde_json::from_value(
        json!({"type": "set", "id": "r1", "mode": "bounding-box", "offset": "6,6,6,6"}),
    )
    .unwrap();
    wrap::run(&mut raw, set).unwrap();
    assert_eq!(
        raw["pages"][0]["layers"][0]["nodes"][0]["wrap"]["mode"],
        "bounding-box"
    );
    transaction::validate_value(&raw).unwrap();
}

#[test]
fn float_canvas_dimensions_render_and_export() {
    // IDML import stores page sizes as floats (612.0); every renderer must accept them.
    let mut raw = v9_document();
    raw["pages"][0]["canvas"]["width"] = json!(200.0);
    raw["pages"][0]["canvas"]["height"] = json!(120.0);
    let dir = std::env::temp_dir().join(format!("pentool-float-canvas-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("doc.pen");
    std::fs::write(&file, serde_json::to_vec(&raw).unwrap()).unwrap();
    for (cmd, out) in [("export", "o.png"), ("export", "o.svg")] {
        let out = dir.join(out);
        let result = Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args([cmd, file.to_str().unwrap(), out.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{cmd}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(out.exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
