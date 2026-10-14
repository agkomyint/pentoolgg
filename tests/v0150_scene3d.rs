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
fn material_overrides_are_atomic_and_inspection_is_paginated() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("painted.obj");
    let mtl = dir.join("painted.mtl");
    std::fs::write(&mtl, "newmtl clay\nKd 0.8 0.2 0.1\nNs 20\n").unwrap();
    std::fs::write(
        &model,
        "mtllib painted.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl clay\nf 1 3 2\n",
    )
    .unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "m",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--external"
    ])
    .status
    .success());

    let inspected = pentool(&[
        "scene3d", d, "inspect", "m", "--offset", "1", "--limit", "1",
    ]);
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let report: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["matches"], 3);
    assert_eq!(report["returned"], 1);
    assert_eq!(report["offset"], 1);
    assert_eq!(report["limit"], 1);
    assert_eq!(report["has_more"], true);
    assert_eq!(report["records"][0]["id"], "material-0");

    assert!(pentool(&[
        "scene3d",
        d,
        "material",
        "m",
        "set",
        "0",
        "--base-color",
        "#2080ffff",
        "--metallic",
        "0.25",
        "--roughness",
        "0.4",
        "--double-sided",
        "true"
    ])
    .status
    .success());
    let stored = read(&doc);
    assert_eq!(
        stored["pages"][0]["layers"][0]["nodes"][0]["material_overrides"]["0"]["roughness"],
        0.4
    );

    let before = std::fs::read(&doc).unwrap();
    let bad = pentool(&[
        "scene3d",
        d,
        "material",
        "m",
        "set",
        "9",
        "--roughness",
        "0.5",
    ]);
    assert!(!bad.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());

    assert!(pentool(&["scene3d", d, "material", "m", "clear", "0"])
        .status
        .success());
    assert!(read(&doc)["pages"][0]["layers"][0]["nodes"][0]
        .get("material_overrides")
        .is_none());
}

#[test]
fn frame_to_fit_stores_a_deterministic_physical_camera() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
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
    let out = pentool(&[
        "scene3d",
        d,
        "frame",
        "m",
        "--azimuth",
        "40",
        "--elevation",
        "25",
        "--focal-length",
        "50",
        "--sensor",
        "36x24",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let first = read(&doc);
    let node = &first["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(node["physical_camera"]["focal_length_mm"], 50.0);
    assert_eq!(node["camera"]["projection"], "perspective");
    assert_eq!(node["camera"]["eye"].as_array().unwrap().len(), 3);
    assert_eq!(node["camera"]["target"].as_array().unwrap().len(), 3);

    assert!(pentool(&[
        "scene3d",
        d,
        "frame",
        "m",
        "--azimuth",
        "40",
        "--elevation",
        "25",
        "--focal-length",
        "50",
        "--sensor",
        "36x24"
    ])
    .status
    .success());
    assert_eq!(first, read(&doc), "framing must be deterministic");

    let before = std::fs::read(&doc).unwrap();
    let bad = pentool(&["scene3d", d, "frame", "m", "--sensor", "36x24"]);
    assert!(!bad.status.success());
    assert_eq!(before, std::fs::read(&doc).unwrap());
}

#[test]
fn accelerated_request_falls_back_explicitly_in_the_default_build() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
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
    let reference = dir.join("reference.png");
    let accelerated = dir.join("accelerated.png");
    assert!(pentool(&["export", d, reference.to_str().unwrap()])
        .status
        .success());
    let out = pentool(&[
        "export",
        d,
        accelerated.to_str().unwrap(),
        "--renderer",
        "accelerated",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diagnostic = String::from_utf8_lossy(&out.stderr);
    assert!(diagnostic.contains("\"renderer_requested\":\"accelerated\""));
    assert!(diagnostic.contains("\"renderer_used\":\"reference\""));
    assert!(diagnostic.contains("without the `gpu` Cargo feature"));
    assert_eq!(
        std::fs::read(reference).unwrap(),
        std::fs::read(accelerated).unwrap()
    );
}

#[test]
fn packages_audit_embedded_models_and_reject_external_sources() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "hero",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l"
    ])
    .status
    .success());

    let package_dir = dir.join("kit");
    assert!(pentool(&[
        "package",
        "init",
        package_dir.to_str().unwrap(),
        "--name",
        "model-kit"
    ])
    .status
    .success());
    let mut asset = read(&doc);
    asset["asset"] = json!({
        "schema": 1, "id": "model-card", "name": "Model card",
        "asset_version": "0.1.0", "kind": "component"
    });
    let asset_path = package_dir.join("assets/model.pen");
    std::fs::write(&asset_path, serde_json::to_vec_pretty(&asset).unwrap()).unwrap();
    let output = dir.join("model.penpkg");
    let packed = pentool(&[
        "package",
        "pack",
        package_dir.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let verified = pentool(&["package", "verify", output.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    let models = &report["package"]["assets"]["model-card"]["models"];
    assert_eq!(models.as_array().unwrap().len(), 1);
    assert_eq!(models[0]["format"], "obj");
    assert_eq!(models[0]["byte_length"], TETRA.len());
    assert_eq!(models[0]["consumers"], json!(["hero"]));

    let digest = asset["models"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    asset["models"][&digest]["storage"] = json!({"kind":"external", "path":"tet.obj"});
    std::fs::write(&asset_path, serde_json::to_vec_pretty(&asset).unwrap()).unwrap();
    let refused = pentool(&[
        "package",
        "pack",
        package_dir.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("must be embedded"));
}

#[test]
fn camera_edits_round_trip_through_shared_undo_and_redo() {
    let dir = scratch();
    let doc = base(&dir);
    let model = dir.join("tet.obj");
    std::fs::write(&model, TETRA).unwrap();
    let d = doc.to_str().unwrap();
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
    let before = read(&doc);
    assert!(pentool(&["scene3d", d, "frame", "m", "--azimuth", "65"])
        .status
        .success());
    let framed = read(&doc);
    assert_ne!(before, framed);
    assert!(pentool(&["undo", d]).status.success());
    assert_eq!(before, read(&doc));
    assert!(pentool(&["redo", d]).status.success());
    assert_eq!(framed, read(&doc));
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

fn png_pixels(path: &Path) -> (u32, u32, Vec<u8>) {
    let img = image::open(path).unwrap().to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

#[test]
fn physical_camera_changes_perspective_and_dof_is_deterministic() {
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
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());

    let mut renders = Vec::new();
    for focal in ["24", "50", "85"] {
        let out = pentool(&["scene3d", d, "camera", "m", "--focal-length", focal]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let png = dir.join(format!("f{focal}.png"));
        assert!(pentool(&["export", d, png.to_str().unwrap()])
            .status
            .success());
        renders.push(png_pixels(&png));
    }
    assert_ne!(renders[0].2, renders[1].2);
    assert_ne!(renders[1].2, renders[2].2);

    // Depth of field: deterministic, and different from the pinhole render.
    let pin = dir.join("pin.png");
    assert!(pentool(&["export", d, pin.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "camera",
        "m",
        "--focal-length",
        "85",
        "--aperture",
        "1.4",
        "--focus-distance",
        "1.0",
        "--lens-samples",
        "8",
    ])
    .status
    .success());
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    assert!(pentool(&["export", d, a.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&["export", d, b.to_str().unwrap()])
        .status
        .success());
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_ne!(png_pixels(&a).2, png_pixels(&pin).2);
}

#[test]
fn inconsistent_fov_and_physical_camera_is_a_validation_error() {
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
        "l",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "camera", "m", "--fov", "20"])
        .status
        .success());
    let before = std::fs::read(&doc).unwrap();
    // The CLI drops the stale fov when a lens is set, so inject the conflict.
    let mut raw = read(&doc);
    raw["pages"][0]["layers"][0]["nodes"][0]["physical_camera"] = json!({"focal_length_mm": 50.0});
    std::fs::write(&doc, serde_json::to_vec(&raw).unwrap()).unwrap();
    let out = pentool(&["export", d, dir.join("x.png").to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("disagree"));
    let _ = before;
}

#[test]
fn depth_of_field_leaves_the_focus_plane_sharp() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    let model = dir.join("quad.obj");
    std::fs::write(
        &model,
        "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3\nf 1 3 4\n",
    )
    .unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "q",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());
    let cam = [
        "scene3d",
        d,
        "camera",
        "q",
        "--eye",
        "0,0,5",
        "--target",
        "0,0,0",
        "--focal-length",
        "50",
    ];
    assert!(pentool(&cam).status.success());
    let sharp = dir.join("sharp.png");
    assert!(pentool(&["export", d, sharp.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "camera",
        "q",
        "--aperture",
        "1.2",
        "--focus-distance",
        "5",
        "--lens-samples",
        "12",
    ])
    .status
    .success());
    let dof = dir.join("dof.png");
    assert!(pentool(&["export", d, dof.to_str().unwrap()])
        .status
        .success());
    let (w, _, a) = png_pixels(&sharp);
    let (_, _, b) = png_pixels(&dof);
    // Interior of the quad (well away from its silhouette): identical.
    for y in 40..60 {
        for x in 45..55 {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                let diff = (a[i + c] as i32 - b[i + c] as i32).abs();
                // View-dependent shading moves with each lens sample; the
                // geometry itself must not blur.
                assert!(diff <= 2, "pixel {x},{y} channel {c}: {diff}");
            }
        }
    }
}

#[test]
fn spot_light_and_shadows_change_the_render_deterministically() {
    let dir = scratch();
    let doc = base(&dir);
    let d = doc.to_str().unwrap();
    // A ground plane with a small blocker above it.
    let obj = "v -2 0 -2\nv 2 0 -2\nv 2 0 2\nv -2 0 2\nf 1 3 2\nf 1 4 3\n\
               v -0.5 1 -0.5\nv 0.5 1 -0.5\nv 0.5 1 0.5\nv -0.5 1 0.5\nf 5 7 6\nf 5 8 7\n";
    let model = dir.join("scene.obj");
    std::fs::write(&model, obj).unwrap();
    assert!(pentool(&[
        "scene3d",
        d,
        "add",
        "s",
        "--model",
        model.to_str().unwrap(),
        "--layer",
        "l",
        "--width",
        "100",
        "--height",
        "100",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "light", "s", "clear"])
        .status
        .success());
    assert!(pentool(&[
        "scene3d",
        d,
        "light",
        "s",
        "add",
        "--kind",
        "spot",
        "--position",
        "0,4,0",
        "--direction",
        "0,-1,0",
        "--outer-deg",
        "40",
        "--intensity",
        "60",
    ])
    .status
    .success());
    assert!(pentool(&["scene3d", d, "camera", "s", "--elevation", "60"])
        .status
        .success());
    let on = dir.join("on.png");
    let on2 = dir.join("on2.png");
    let off = dir.join("off.png");
    assert!(pentool(&["export", d, on.to_str().unwrap()])
        .status
        .success());
    assert!(pentool(&["export", d, on2.to_str().unwrap()])
        .status
        .success());
    assert_eq!(std::fs::read(&on).unwrap(), std::fs::read(&on2).unwrap());
    assert!(pentool(&["scene3d", d, "set", "s", "--shadows", "false"])
        .status
        .success());
    assert!(pentool(&["export", d, off.to_str().unwrap()])
        .status
        .success());
    let sum = |p: &Path| png_pixels(p).2.iter().map(|b| *b as u64).sum::<u64>();
    assert!(sum(&on) < sum(&off), "shadows must darken the ground");
}
