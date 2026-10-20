//! `--preview` on mutating commands: one call edits the document and returns a
//! size-bounded PNG of the changed page, without changing what is written.
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-preview-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn document(&self) -> PathBuf {
        let path = self.0.join("doc.pen");
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pentool(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pentool"));
    command.args(args).env_remove("PENTOOL_PREVIEW");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn rect(doc: &Path, id: &str, extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "shape".to_string(),
        doc.display().to_string(),
        "rect".into(),
        id.into(),
        "--layer".into(),
        "content".into(),
        "--x".into(),
        "10".into(),
        "--y".into(),
        "10".into(),
        "--w".into(),
        "50".into(),
        "--h".into(),
        "50".into(),
        "--fill".into(),
        "#ff0000".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn run(args: &[String], env: &[(&str, &str)]) -> Value {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    json(&pentool(&args, env))
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn preview_matches_the_committed_revision_and_is_size_bounded() {
    let space = Workspace::new("commit");
    let doc = space.document();
    let result = run(&rect(&doc, "box", &["--preview", "--no-focus"]), &[]);
    let preview = &result["preview"];
    assert!(preview.get("focus").is_none() && preview.get("thumbnail").is_none());
    assert_eq!(preview["revision"], result["revision_after"]);
    assert_eq!(
        preview["revision"].as_str().unwrap(),
        sha256(&fs::read(&doc).unwrap())
    );
    assert_eq!(preview["page"], "page-1");
    let path = PathBuf::from(preview["path"].as_str().unwrap());
    assert!(path.starts_with(space.0.join(".pentool").join("preview")));
    let image = image::open(&path).unwrap();
    assert_eq!(image.width().max(image.height()), 1024);
    assert_eq!(preview["width"], image.width());
    assert_eq!(preview["height"], image.height());

    // The preview is the normal render: same pixels as an export at its scale.
    let exported = space.0.join("export.png");
    let scale = preview["scale"].as_f64().unwrap().to_string();
    let out = pentool(
        &[
            "export",
            doc.to_str().unwrap(),
            exported.to_str().unwrap(),
            "--scale",
            &scale,
        ],
        &[],
    );
    json(&out);
    assert_eq!(
        image::open(&exported).unwrap().to_rgba8().into_raw(),
        image.to_rgba8().into_raw()
    );
}

#[test]
fn composite_documents_preview_through_the_composite_renderer() {
    let space = Workspace::new("composite");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    let result = json(&pentool(
        &[
            "effect",
            "add",
            path,
            "card",
            "shadow",
            "--kind",
            "drop-shadow",
            "--preview",
            "--preview-size",
            "400",
            "--no-focus",
        ],
        &[],
    ));
    // Commands with their own result shape get "preview" at the top level.
    assert_eq!(result["result"]["node"], "card");
    let preview = &result["preview"];
    assert_eq!(preview["revision"], result["change"]["revision_after"]);
    let image = image::open(preview["path"].as_str().unwrap()).unwrap();
    assert_eq!(image.width().max(image.height()), 400);
    let exported = space.0.join("export.png");
    let scale = preview["scale"].as_f64().unwrap().to_string();
    json(&pentool(
        &[
            "export",
            path,
            exported.to_str().unwrap(),
            "--scale",
            &scale,
        ],
        &[],
    ));
    assert_eq!(
        image::open(&exported).unwrap().to_rgba8().into_raw(),
        image.to_rgba8().into_raw()
    );
}

#[test]
fn read_commands_print_the_same_bytes_with_previews_on() {
    let space = Workspace::new("read");
    let doc = space.document();
    let args = ["tree", doc.to_str().unwrap(), "--limit", "2"];
    let plain = pentool(&args, &[]);
    let with = pentool(&args, &[("PENTOOL_PREVIEW", "1")]);
    assert!(plain.status.success() && with.status.success());
    assert_eq!(plain.stdout, with.stdout);
    assert!(!space.0.join(".pentool").join("preview").exists());
}

#[test]
fn no_flag_means_no_preview_and_no_files() {
    let space = Workspace::new("off");
    let doc = space.document();
    let result = run(&rect(&doc, "box", &[]), &[]);
    assert!(result.get("preview").is_none());
    assert!(!space.0.join(".pentool").join("preview").exists());
}

#[test]
fn environment_turns_previews_on_and_size_is_selectable() {
    let space = Workspace::new("env");
    let doc = space.document();
    let result = run(
        &rect(&doc, "box", &["--preview-size", "200", "--no-focus"]),
        &[("PENTOOL_PREVIEW", "1")],
    );
    let image = image::open(result["preview"]["path"].as_str().unwrap()).unwrap();
    assert_eq!(image.width().max(image.height()), 200);
}

#[test]
fn dry_run_previews_the_plan_without_writing() {
    let space = Workspace::new("dry");
    let doc = space.document();
    let before = fs::read(&doc).unwrap();
    let out_path = space.0.join("plan.png");
    let result = run(
        &rect(
            &doc,
            "box",
            &[
                "--dry-run",
                "--preview",
                "--preview-out",
                out_path.to_str().unwrap(),
            ],
        ),
        &[],
    );
    assert_eq!(fs::read(&doc).unwrap(), before, "dry run left the document");
    assert_eq!(result["preview"]["path"], out_path.display().to_string());
    assert_eq!(result["preview"]["revision"], result["revision_after"]);
    // The planned red square is in the preview.
    let image = image::open(&out_path).unwrap().to_rgba8();
    let scale = result["preview"]["scale"].as_f64().unwrap();
    let at = (35.0 * scale) as u32;
    assert_eq!(image.get_pixel(at, at).0, [255, 0, 0, 255]);
}

#[test]
fn preview_shows_the_changed_page() {
    let space = Workspace::new("page");
    let doc = space.document();
    let added = pentool(
        &[
            "page",
            doc.to_str().unwrap(),
            "add",
            "second",
            "--width",
            "300",
            "--height",
            "100",
        ],
        &[],
    );
    json(&added);
    // Renaming the second page changes only that page, so it is the one shown.
    let result = json(&pentool(
        &[
            "page",
            doc.to_str().unwrap(),
            "rename",
            "second",
            "Second",
            "--preview",
        ],
        &[],
    ));
    assert_eq!(result["preview"]["page"], "second");
    let image = image::open(result["preview"]["path"].as_str().unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (1024, 341));
    // --page overrides the detected page.
    let result = json(&pentool(
        &[
            "page",
            doc.to_str().unwrap(),
            "rename",
            "second",
            "Other",
            "--preview",
            "--page",
            "page-1",
        ],
        &[],
    ));
    assert_eq!(result["preview"]["page"], "page-1");
}

#[test]
fn a_failed_preview_never_fails_the_commit() {
    let space = Workspace::new("fail");
    let doc = space.document();
    let blocker = space.0.join("file");
    fs::write(&blocker, b"not a directory").unwrap();
    let out_path = blocker.join("preview.png");
    let result = run(
        &rect(
            &doc,
            "box",
            &["--preview", "--preview-out", out_path.to_str().unwrap()],
        ),
        &[],
    );
    assert_eq!(result["changed"], true);
    assert!(result["preview"]["error"].as_str().is_some());
    assert_eq!(
        result["revision_after"].as_str().unwrap(),
        sha256(&fs::read(&doc).unwrap())
    );
}

#[test]
fn preview_rejects_an_out_of_range_size_before_writing() {
    let space = Workspace::new("size");
    let doc = space.document();
    let before = fs::read(&doc).unwrap();
    let args: Vec<String> = rect(&doc, "box", &["--preview", "--preview-size", "0"]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = pentool(&args, &[]);
    assert!(!out.status.success());
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn preview_directory_is_bounded() {
    let space = Workspace::new("bound");
    let doc = space.document();
    for index in 0..36 {
        let id = format!("box{index}");
        run(
            &rect(&doc, &id, &["--preview", "--preview-size", "16"]),
            &[],
        );
    }
    let kept = fs::read_dir(space.0.join(".pentool").join("preview"))
        .unwrap()
        .count();
    assert_eq!(kept, 32);
}

fn export(doc: &Path, out: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["export", doc.to_str().unwrap(), out.to_str().unwrap()];
    args.extend_from_slice(extra);
    pentool(&args, &[])
}

#[test]
fn export_max_size_picks_the_scale_for_the_long_edge() {
    let space = Workspace::new("max-size");
    let doc = space.document(); // 320 x 240
    for (size, short) in [(200, 150), (333, 250), (1000, 750), (2560, 1920)] {
        let out = space.0.join(format!("max-{size}.png"));
        let result = json(&export(&doc, &out, &["--max-size", &size.to_string()]));
        assert_eq!(result["max_size"], size);
        let image = image::open(&out).unwrap();
        assert_eq!((image.width(), image.height()), (size, short), "{result}");
    }
    // Same pixels as the equivalent --scale export.
    let by_scale = space.0.join("scale.png");
    let plain = json(&export(&doc, &by_scale, &["--scale", "0.625"]));
    assert!(plain.get("scale").is_none() && plain.get("max_size").is_none());
    assert_eq!(
        fs::read(&by_scale).unwrap(),
        fs::read(space.0.join("max-200.png")).unwrap()
    );
}

#[test]
fn export_max_size_works_for_composite_documents() {
    let space = Workspace::new("max-size-composite");
    let doc = space.document();
    json(&pentool(
        &["migrate", doc.to_str().unwrap(), "--target", "6"],
        &[],
    ));
    let out = space.0.join("out.png");
    json(&export(&doc, &out, &["--max-size", "500"]));
    let image = image::open(&out).unwrap();
    assert_eq!((image.width(), image.height()), (500, 375));
}

#[test]
fn export_max_size_rejects_bad_bounds_before_writing() {
    let space = Workspace::new("max-size-bad");
    let doc = space.document();
    let out = space.0.join("out.png");
    for (args, message) in [
        (&["--max-size", "10"][..], "[limit-exceeded]"),
        (&["--max-size", "0"][..], "--max-size must be in"),
        (&["--max-size", "9000"][..], "--max-size must be in"),
        (
            &["--max-size", "100", "--scale", "2"][..],
            "cannot be used with",
        ),
    ] {
        let output = export(&doc, &out, args);
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert!(!out.exists());
    }
}

/// The `(x, y, width, height)` pixels of `image` as raw RGBA.
fn crop(image: &image::DynamicImage, x: u32, y: u32, width: u32, height: u32) -> Vec<u8> {
    image.crop_imm(x, y, width, height).to_rgba8().into_raw()
}

#[test]
fn preview_focuses_on_the_changed_objects_with_a_page_thumbnail() {
    let space = Workspace::new("focus");
    let doc = space.document();
    let result = run(&rect(&doc, "box", &["--preview"]), &[]);
    let preview = &result["preview"];
    // 50 x 50 at (10, 10), padded by 8 on each side.
    assert_eq!(
        preview["focus"],
        serde_json::json!({"ids": ["box"], "x": 2.0, "y": 2.0, "width": 66.0, "height": 66.0})
    );
    assert_eq!(preview["scale"], 8.0);
    let image = image::open(preview["path"].as_str().unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (528, 528));
    assert_eq!(preview["width"], 528);
    let thumbnail = &preview["thumbnail"];
    let small = image::open(thumbnail["path"].as_str().unwrap()).unwrap();
    assert_eq!((small.width(), small.height()), (256, 192));
    assert_eq!(thumbnail["width"], 256);

    // The focused image is the matching part of a full export at its scale.
    let exported = space.0.join("export.png");
    json(&export(&doc, &exported, &["--scale", "8"]));
    let full = image::open(&exported).unwrap();
    assert_eq!(crop(&full, 16, 16, 528, 528), image.to_rgba8().into_raw());
}

#[test]
fn focus_names_objects_and_reports_unknown_ones_as_preview_errors() {
    let space = Workspace::new("focus-explicit");
    let doc = space.document();
    let result = run(
        &rect(
            &doc,
            "box",
            &["--preview", "--focus", "box", "--focus", "card"],
        ),
        &[],
    );
    assert_eq!(
        result["preview"]["focus"]["ids"],
        serde_json::json!(["box", "card"])
    );
    let result = run(
        &rect(&doc, "box2", &["--preview", "--focus", "missing"]),
        &[],
    );
    let error = result["preview"]["error"].as_str().unwrap();
    assert!(error.contains("cannot focus on `missing`"), "{error}");
    assert!(result["revision_after"].is_string());
    let output = pentool(
        &["tree", doc.to_str().unwrap(), "--focus", "a", "--no-focus"],
        &[],
    );
    assert!(!output.status.success());
}

#[test]
fn large_changes_preview_the_whole_page() {
    let space = Workspace::new("focus-large");
    let doc = space.document();
    let mut args = rect(&doc, "cover", &["--preview"]);
    for (flag, value) in [("--x", "0"), ("--y", "0"), ("--w", "320"), ("--h", "240")] {
        let at = args.iter().position(|a| a == flag).unwrap();
        args[at + 1] = value.into();
    }
    let preview = &run(&args, &[])["preview"];
    assert!(preview.get("focus").is_none());
    assert_eq!(preview["width"], 1024);
}

#[test]
fn composite_focus_is_a_crop_of_the_composite_render() {
    let space = Workspace::new("focus-composite");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    let result = run(&rect(&doc, "box", &["--preview"]), &[]);
    let preview = &result["preview"];
    assert_eq!(preview["focus"]["ids"], serde_json::json!(["box"]));
    let image = image::open(preview["path"].as_str().unwrap()).unwrap();
    let scale = preview["scale"].as_f64().unwrap();
    let exported = space.0.join("export.png");
    json(&export(&doc, &exported, &["--scale", &scale.to_string()]));
    let full = image::open(&exported).unwrap();
    let origin = (2.0 * scale).floor() as u32;
    assert_eq!(
        crop(&full, origin, origin, image.width(), image.height()),
        image.to_rgba8().into_raw()
    );
}
