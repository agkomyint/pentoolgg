use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TETRA: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\n";
const OVERLAPPING_UV_TRIANGLES: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nvt 0 0\nvt 1 0\nvt 0 1\nvt 0.2 0.2\nvt 0.4 0.2\nvt 0.2 0.4\nf 1/1 2/2 3/3\nf 2/4 4/5 3/6\n";

fn scratch() -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pentool-v0160-uv-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn uv_island_pack_places_islands_inside_unit_square_with_padding() {
    let dir = scratch();
    let document = dir.join("design.pen");
    std::fs::write(
        &document,
        serde_json::to_vec_pretty(&pentool::scene::new_document(120, 120)).unwrap(),
    )
    .unwrap();
    let model = dir.join("tetra.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = document.to_str().unwrap();
    let m = model.to_str().unwrap();
    for args in [
        vec!["migrate", d, "--target", "10"],
        vec![
            "scene3d", d, "add", "model", "--model", m, "--layer", "layer-1",
        ],
        vec!["migrate", d, "--target", "11"],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "project",
            "--projection",
            "box",
        ],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "island",
            "add",
            "--id",
            "front",
            "--triangles",
            "0,1",
        ],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "island",
            "add",
            "--id",
            "back",
            "--triangles",
            "2,3",
        ],
    ] {
        let output = run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let before_dry_run = std::fs::read(&document).unwrap();
    let dry_run = run(&[
        "scene3d",
        d,
        "--dry-run",
        "uv",
        "model",
        "island",
        "pack",
        "--padding",
        "0.02",
    ]);
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert_eq!(std::fs::read(&document).unwrap(), before_dry_run);

    let packed = run(&[
        "scene3d",
        d,
        "uv",
        "model",
        "island",
        "pack",
        "--padding",
        "0.02",
    ]);
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let after = read(&document);
    let digest = after["pages"][0]["layers"][0]["nodes"][0]["model"]
        .as_str()
        .unwrap();
    let report = pentool::scene3d::node::load_model(&after, &dir, digest).unwrap();
    let uv_report = pentool::scene3d::uv::show_with_overlays(
        &report,
        after["models"][digest].get("uv").unwrap(),
    )
    .unwrap();
    let islands = uv_report["primitives"][0]["islands"].as_array().unwrap();
    assert_eq!(islands.len(), 2);
    for island in islands {
        let min = island["uv_bounds"]["min"].as_array().unwrap();
        let max = island["uv_bounds"]["max"].as_array().unwrap();
        for axis in 0..2 {
            assert!(min[axis].as_f64().unwrap() >= 0.0199, "{island}");
            assert!(max[axis].as_f64().unwrap() <= 0.9801, "{island}");
        }
    }
    assert!(
        uv_report["primitives"][0]["island_overlap_candidates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{uv_report}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uv_show_cli_reports_triangle_interior_overlap_without_mutating_document() {
    let dir = scratch();
    let document = dir.join("overlap.pen");
    std::fs::write(
        &document,
        serde_json::to_vec_pretty(&pentool::scene::new_document(120, 120)).unwrap(),
    )
    .unwrap();
    let model = dir.join("overlap.obj");
    std::fs::write(&model, OVERLAPPING_UV_TRIANGLES).unwrap();
    let d = document.to_str().unwrap();
    let m = model.to_str().unwrap();
    for args in [
        vec!["migrate", d, "--target", "10"],
        vec![
            "scene3d", d, "add", "model", "--model", m, "--layer", "layer-1",
        ],
        vec!["migrate", d, "--target", "11"],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "island",
            "add",
            "--id",
            "outer",
            "--triangles",
            "0",
        ],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "island",
            "add",
            "--id",
            "inner",
            "--triangles",
            "1",
        ],
    ] {
        let output = run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let before_show = std::fs::read(&document).unwrap();
    let shown = run(&["scene3d", d, "uv", "model", "show"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let report: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        report["primitives"][0]["island_overlap_candidates"],
        json!([["outer", "inner"]])
    );
    assert_eq!(
        report["primitives"][0]["island_overlap_analysis_truncated"],
        false
    );
    assert_eq!(std::fs::read(&document).unwrap(), before_show);
    std::fs::remove_dir_all(dir).unwrap();
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn editable_uv_islands_are_transactional_and_survive_reload() {
    let dir = scratch();
    let document = dir.join("design.pen");
    std::fs::write(
        &document,
        serde_json::to_vec_pretty(&pentool::scene::new_document(120, 120)).unwrap(),
    )
    .unwrap();
    let model = dir.join("tetra.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = document.to_str().unwrap();
    let m = model.to_str().unwrap();
    for args in [
        vec!["migrate", d, "--target", "10"],
        vec![
            "scene3d", d, "add", "model", "--model", m, "--layer", "layer-1",
        ],
        vec!["migrate", d, "--target", "11"],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "project",
            "--projection",
            "box",
        ],
        vec![
            "scene3d",
            d,
            "uv",
            "model",
            "island",
            "add",
            "--id",
            "front",
            "--triangles",
            "0,1",
            "--scale",
            "-0.5,0.5",
            "--offset",
            "0.1,0",
            "--rotate",
            "15",
        ],
    ] {
        let output = run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let saved = read(&document);
    let digest = saved["pages"][0]["layers"][0]["nodes"][0]["model"]
        .as_str()
        .unwrap();
    let overlay = &saved["models"][digest]["uv"]["0"];
    assert_eq!(overlay["islands"][0]["id"], "front");
    assert_eq!(overlay["islands"][0]["triangles"], json!([0, 1]));
    assert_eq!(
        overlay["islands"][0]["transform"]["scale"],
        json!([-0.5, 0.5])
    );

    let listed = run(&["scene3d", d, "uv", "model", "island", "list"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&listed.stdout).unwrap()["islands"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let shown = run(&["scene3d", d, "uv", "model", "show"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let report: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(
        report["primitives"][0]["texel_density"][0]["texture_resolution_px"],
        json!([2048, 2048])
    );
    assert!(
        report["primitives"][0]["texel_density"][0]["texel_density_px_per_model_unit"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    assert_eq!(report["primitives"][0]["islands"][0]["id"], "front");
    assert_eq!(
        report["primitives"][0]["islands"][0]["below_minimum_texel_density"],
        Value::Null,
        "without a user threshold the report should not judge density adequacy"
    );
    assert_eq!(
        report["primitives"][0]["islands"][0]["texture_resolution_px"],
        json!([2048, 2048])
    );
    let density_warn = run(&[
        "scene3d",
        d,
        "uv",
        "model",
        "show",
        "--resolution",
        "1024",
        "--minimum-density",
        "999999",
    ]);
    assert!(
        density_warn.status.success(),
        "{}",
        String::from_utf8_lossy(&density_warn.stderr)
    );
    let warning_report: Value = serde_json::from_slice(&density_warn.stdout).unwrap();
    assert_eq!(
        warning_report["primitives"][0]["islands"][0]["below_minimum_texel_density"],
        json!(true)
    );
    assert!(report["primitives"][0]["islands"][0]["mirrored"]
        .as_bool()
        .unwrap());
    assert!(report["primitives"][0]["islands"][0]["uv_bounds"].is_object());

    let before_bad_edit = std::fs::read(&document).unwrap();
    let bad = run(&[
        "scene3d",
        d,
        "uv",
        "model",
        "island",
        "set",
        "front",
        "--triangles",
        "999",
    ]);
    assert!(!bad.status.success());
    assert_eq!(
        std::fs::read(&document).unwrap(),
        before_bad_edit,
        "invalid triangle assignment must not mutate the file"
    );

    let set = run(&[
        "scene3d",
        d,
        "uv",
        "model",
        "island",
        "set",
        "front",
        "--scale",
        "0.75,0.75",
    ]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    assert_eq!(
        read(&document)["models"][digest]["uv"]["0"]["islands"][0]["transform"]["scale"],
        json!([0.75, 0.75])
    );
    let removed = run(&["scene3d", d, "uv", "model", "island", "remove", "front"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(read(&document)["models"][digest]["uv"]["0"]
        .get("islands")
        .is_none());
}
