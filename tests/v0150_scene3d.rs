//! v0.15.0: version 10 scene3d nodes, the models map, migration, and the CLI.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

const TETRA: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\n";

fn scratch() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pentool-scene3d-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn pentool(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn base(dir: &Path) -> PathBuf {
    let doc = json!({
        "format": "pentool", "version": 4, "name": "t",
        "pages": [{"id": "p", "name": "P",
            "canvas": {"width": 120, "height": 120, "background": "#ffffff"},
            "layers": [{"id": "l", "name": "L", "visible": true, "locked": false, "nodes": []}]}],
        "styles": {}
    });
    let path = dir.join("d.pen");
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let out = pentool(&["migrate", path.to_str().unwrap(), "--target", "10"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    path
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn add_camera_light_and_export_render_the_model() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["report"]["triangles"], 4);
    assert_eq!(read(&doc)["version"], 10);

    assert!(pentool(&["scene3d", d, "camera", "m", "--azimuth", "30"])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "light",
        "m",
        "add",
        "--kind",
        "point",
        "--position",
        "1,2,3"
    ])
    .status
    .success());
    let png = dir.join("o.png");
    let out = pentool(&["export", d, png.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = std::fs::read(&png).unwrap();
    assert!(pentool(&["export", d, png.to_str().unwrap()])
        .status
        .success());
    assert_eq!(
        first,
        std::fs::read(&png).unwrap(),
        "export must be deterministic"
    );
}

#[test]
fn dry_run_and_bad_input_leave_the_file_untouched() {
    let dir = scratch();
    let doc = base(&dir);
    let before = std::fs::read(&doc).unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "--dry-run",
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
    ]);
    assert!(out.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
    let bad = dir.join("bad.obj");
    std::fs::write(&bad, "f 1 2 3\n").unwrap();
    let out = pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        bad.to_str().unwrap(),
        "--layer",
        "l",
    ]);
    assert!(!out.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
}

#[test]
fn downgrade_is_refused_while_3d_is_used_and_allowed_when_empty() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let out = pentool(&["migrate", d, "--target", "9"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("scene3d"));
    let empty = base(&scratch());
    assert!(
        pentool(&["migrate", empty.to_str().unwrap(), "--target", "9"])
            .status
            .success()
    );
    assert_eq!(read(&empty)["version"], 9);
}

#[test]
fn scene3d_node_in_a_v9_document_is_rejected_with_a_hint() {
    let dir = scratch();
    let doc = base(&dir);
    assert!(
        pentool(&["migrate", doc.to_str().unwrap(), "--target", "9"])
            .status
            .success()
    );
    let mut raw = read(&doc);
    raw["pages"][0]["layers"][0]["nodes"] = json!([{
        "id": "m", "kind": "scene3d", "x": 0, "y": 0, "width": 10, "height": 10,
        "model": "sha256:00"
    }]);
    let path = dir.join("v9.pen");
    std::fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
    let out = pentool(&[
        "export",
        path.to_str().unwrap(),
        dir.join("x.png").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("migrate --target 10"));
}

#[test]
fn uv_project_is_an_overlay_and_layout_exports() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());
    let shown = pentool(&["scene3d", d, "uv", "m", "show"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["primitives"][0]["uv_sets"], 0);

    let out = pentool(&[
        "scene3d",
        d,
        "uv",
        "m",
        "project",
        "--projection",
        "box",
        "--scale",
        "2,2",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = read(&doc);
    let digest = after["pages"][0]["layers"][0]["nodes"][0]["model"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(after["models"][&digest]["uv"]["*"]["projection"], "box");
    // The source model bytes are unchanged by the overlay.
    assert!(crate_digest_matches(&after, &digest));

    let svg = dir.join("uv.svg");
    let png = dir.join("uv.png");
    for out in [&svg, &png] {
        let r = pentool(&[
            "scene3d",
            d,
            "uv",
            "m",
            "layout",
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    }
    assert!(std::fs::read_to_string(&svg).unwrap().contains("<path"));
    assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG");

    let bad = pentool(&[
        "scene3d",
        d,
        "uv",
        "m",
        "project",
        "--primitive",
        "9",
        "--projection",
        "planar",
    ]);
    assert!(!bad.status.success());
    assert_eq!(read(&doc), after, "failed projection must not write");

    assert!(pentool(&["scene3d", d, "uv", "m", "clear"])
        .status
        .success());
    assert!(read(&doc)["models"][&digest].get("uv").is_none());
}

fn crate_digest_matches(doc: &Value, digest: &str) -> bool {
    use base64::Engine as _;
    let data = doc["models"][digest]["storage"]["data"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    pentool::resource::sha256(&bytes) == digest
}
