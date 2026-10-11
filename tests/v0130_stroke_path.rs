use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pentool-v0130-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name);
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        path.to_string_lossy().into_owned()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn semantic_paths_are_deterministic_and_report_compilation_counts() {
    let workspace = Workspace::new();
    let a = workspace.document("a.pen");
    let b = workspace.document("b.pen");
    for document in [&a, &b] {
        ok(&[
            "raster", document, "add", "paint", "--width", "128", "--height", "128",
        ]);
        let result = ok(&[
            "raster",
            document,
            "stroke-path",
            "paint",
            "--path",
            "M10 100 Q40 10 64 64 C80 100 100 20 118 30 A8 8 0 0 1 110 38",
            "--brush",
            r#"{"size":6,"spacing":0.25,"flow":0.8}"#,
            "--curves",
            r#"{"pressure":[[0,0.2],[0.5,1],[1,0.4]]}"#,
            "--color",
            "#8B5A3C",
            "--seed",
            "42",
        ]);
        assert_eq!(result["result"]["source"]["segments"], 3);
        assert_eq!(result["result"]["generated"]["strokes"], 1);
        assert!(result["result"]["generated"]["points"].as_u64().unwrap() > 4);
    }
    assert_eq!(
        ok(&["raster", &a, "info", "paint"])["tile_map_sha256"],
        ok(&["raster", &b, "info", "paint"])["tile_map_sha256"]
    );
}

#[test]
fn property_curves_compile_to_atomic_ordinary_strokes() {
    let workspace = Workspace::new();
    let document = workspace.document("curves.pen");
    ok(&[
        "raster", &document, "add", "paint", "--width", "128", "--height", "128",
    ]);
    let result = ok(&[
        "raster",
        &document,
        "stroke-path",
        "paint",
        "--path",
        "M10 64 C35 5 90 120 118 64",
        "--brush",
        r#"{"size":8,"spacing":0.4,"flow":0.7}"#,
        "--curves",
        r##"{"size":[[0,3],[0.5,14],[1,5]],"flow":[[0,0.2],[1,0.9]],"opacity":[[0,0.4],[1,1]],"rotation":[[0,-20],[1,30]],"color":[[0,"#2040FF"],[1,"#FF8020"]]}"##,
    ]);
    assert!(result["result"]["generated"]["strokes"].as_u64().unwrap() > 1);
    assert_eq!(result["dry_run"], false);
    assert_eq!(
        ok(&["raster", &document, "verify", "paint", "--replay"])["ok"],
        true
    );
}

#[test]
fn invalid_paths_curves_and_dry_runs_never_mutate() {
    let workspace = Workspace::new();
    let document = workspace.document("atomic.pen");
    ok(&[
        "raster", &document, "add", "paint", "--width", "64", "--height", "64",
    ]);
    let before = fs::read(&document).unwrap();
    for extra in [
        vec!["--path", "m0 0 l10 10"],
        vec!["--path", "M0 0 L10 10 Z"],
        vec![
            "--path",
            "M0 0 L10 10",
            "--curves",
            r#"{"size":[[0,2],[0,3],[1,4]]}"#,
        ],
    ] {
        let mut args = vec!["raster", document.as_str(), "stroke-path", "paint"];
        args.extend(extra);
        assert!(!run(&args).status.success());
        assert_eq!(fs::read(&document).unwrap(), before);
    }
    let result = ok(&[
        "raster",
        &document,
        "--dry-run",
        "stroke-path",
        "paint",
        "--path",
        "M0 0 L20 20",
    ]);
    assert_eq!(result["dry_run"], true);
    assert_eq!(fs::read(&document).unwrap(), before);
}
