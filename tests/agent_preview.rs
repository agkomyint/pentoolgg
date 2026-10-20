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

#[test]
fn preview_command_looks_without_writing() {
    let space = Workspace::new("view");
    let doc = space.document();
    let original = fs::read(&doc).unwrap();
    let path = doc.to_str().unwrap();
    let result = json(&pentool(&["preview", path], &[]));
    let preview = &result["preview"];
    assert_eq!(preview["revision"].as_str().unwrap(), sha256(&original));
    assert_eq!(preview["page"], "page-1");
    assert!(preview.get("focus").is_none() && preview.get("dry_run").is_none());
    let image = image::open(preview["path"].as_str().unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (1024, 768));
    assert_eq!(fs::read(&doc).unwrap(), original);

    let out = space.0.join("card.png");
    let result = json(&pentool(
        &[
            "preview",
            path,
            "--focus",
            "title",
            "--max-size",
            "300",
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    ));
    let preview = &result["preview"];
    assert_eq!(preview["path"], out.display().to_string());
    assert_eq!(preview["focus"]["ids"], serde_json::json!(["title"]));
    let image = image::open(&out).unwrap();
    assert!(image.width().max(image.height()) <= 300);
    assert!(image::open(space.0.join("card-page.png")).is_ok());
    assert_eq!(fs::read(&doc).unwrap(), original);

    for args in [
        &["preview", path, "--focus", "missing"][..],
        &["preview", path, "--max-size", "0"][..],
        &["preview", path, "--page", "nope"][..],
    ] {
        let output = pentool(args, &[]);
        assert!(!output.status.success(), "{args:?}");
    }
    assert_eq!(fs::read(&doc).unwrap(), original);
}

#[test]
fn annotate_labels_objects_on_the_preview_only() {
    let space = Workspace::new("annotate");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    let plain = json(&pentool(&["preview", path], &[]));
    let plain_bytes = fs::read(plain["preview"]["path"].as_str().unwrap()).unwrap();
    let out = space.0.join("labels.png");
    let result = json(&pentool(
        &[
            "preview",
            path,
            "--annotate",
            "--out",
            out.to_str().unwrap(),
        ],
        &[],
    ));
    let annotations = &result["preview"]["annotations"];
    let labels = annotations["labels"].as_array().unwrap();
    assert!(!labels.is_empty());
    assert_eq!(annotations["truncated"], false);
    let ids: Vec<&str> = labels.iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"title"), "{ids:?}");
    let annotated = image::open(&out).unwrap().to_rgba8();
    let plain = image::load_from_memory(&plain_bytes).unwrap().to_rgba8();
    assert_eq!(annotated.dimensions(), plain.dimensions());
    assert_ne!(annotated.as_raw(), plain.as_raw());
    // The last label is drawn on top, in its color on its left edge.
    let first = labels.last().unwrap();
    let color = first["color"].as_str().unwrap();
    let rgb = [1, 3, 5].map(|i| u8::from_str_radix(&color[i..i + 2], 16).unwrap());
    let (x, y) = (
        first["x"].as_f64().unwrap() as u32 + 1,
        (first["y"].as_f64().unwrap() + first["height"].as_f64().unwrap() / 2.0) as u32,
    );
    assert_eq!(annotated.get_pixel(x, y).0[..3], rgb);

    // Exports never carry the overlay.
    let a = space.0.join("a.png");
    let b = space.0.join("b.png");
    json(&export(&doc, &a, &[]));
    json(&export(&doc, &b, &["--annotate"]));
    assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
}

#[test]
fn annotate_with_focus_labels_the_focused_objects_in_crop_pixels() {
    let space = Workspace::new("annotate-focus");
    let doc = space.document();
    let result = run(&rect(&doc, "box", &["--preview", "--annotate"]), &[]);
    let preview = &result["preview"];
    let labels = preview["annotations"]["labels"].as_array().unwrap();
    assert_eq!(labels.len(), 1);
    // The 50x50 box at (10,10), focused at (2,2) and scale 8.
    assert_eq!(labels[0]["id"], "box");
    assert_eq!(
        [
            &labels[0]["x"],
            &labels[0]["y"],
            &labels[0]["width"],
            &labels[0]["height"]
        ],
        [64.0, 64.0, 400.0, 400.0]
    );
    // The page thumbnail stays clean.
    let thumbnail = preview["thumbnail"]["path"].as_str().unwrap();
    let clean = json(&pentool(
        &["preview", doc.to_str().unwrap(), "--max-size", "256"],
        &[],
    ));
    assert_eq!(
        image::open(thumbnail).unwrap().to_rgba8().as_raw(),
        image::open(clean["preview"]["path"].as_str().unwrap())
            .unwrap()
            .to_rgba8()
            .as_raw()
    );
}

#[test]
fn compare_counts_the_changed_pixels_and_writes_a_diff() {
    let space = Workspace::new("compare");
    let doc = space.document();
    let original = fs::read(&doc).unwrap();
    let path = doc.to_str().unwrap();
    let result = run(
        &rect(&doc, "box", &["--compare", "before", "--no-focus"]),
        &[],
    );
    let preview = &result["preview"];
    let compare = &preview["compare"];
    assert_eq!(compare["against"], "before");
    assert_eq!(compare["revision"].as_str().unwrap(), sha256(&original));
    let scale = preview["scale"].as_f64().unwrap();
    // The 50x50 box at (10,10), give or take antialiased edges.
    let bbox = &compare["changed_bbox"];
    for (key, expected) in [("x", 10.0), ("y", 10.0), ("width", 50.0), ("height", 50.0)] {
        let got = bbox[key].as_f64().unwrap();
        assert!((got - expected * scale).abs() <= 2.0, "{key}: {got} {bbox}");
    }
    let changed = compare["changed_pixels"].as_u64().unwrap();
    let total = preview["width"].as_u64().unwrap() * preview["height"].as_u64().unwrap();
    assert!(changed > 0 && changed <= bbox["width"].as_u64().unwrap().pow(2));
    let pct = compare["changed_pct"].as_f64().unwrap();
    assert!((pct - 100.0 * changed as f64 / total as f64).abs() < 0.001);
    let diff = image::open(compare["diff"]["path"].as_str().unwrap())
        .unwrap()
        .to_rgba8();
    let red = diff.pixels().filter(|p| p.0 == [255, 0, 0, 255]).count();
    assert_eq!(red as u64, changed);

    // The read-only command compares with a history revision, `before` (the
    // last recorded change's start) or a file, and never writes the document.
    let after = fs::read(&doc).unwrap();
    let by_revision = json(&pentool(
        &["preview", path, "--compare", &sha256(&original)],
        &[],
    ));
    let by_before = json(&pentool(&["preview", path, "--compare", "before"], &[]));
    assert_eq!(by_revision["preview"]["compare"]["changed_pixels"], changed);
    assert_eq!(
        by_before["preview"]["compare"]["changed_bbox"],
        by_revision["preview"]["compare"]["changed_bbox"]
    );
    let same = space.0.join("same.pen");
    fs::write(&same, &after).unwrap();
    let unchanged = json(&pentool(
        &["preview", path, "--compare", same.to_str().unwrap()],
        &[],
    ));
    let compare = &unchanged["preview"]["compare"];
    assert_eq!(compare["changed_pixels"], 0);
    assert_eq!(compare["changed_bbox"], Value::Null);
    assert!(compare.get("diff").is_none());
    assert_eq!(fs::read(&doc).unwrap(), after);

    let fresh = space.0.join("fresh.pen");
    fs::write(&fresh, &original).unwrap();
    for args in [
        &["preview", path, "--compare", "missing.pen"][..],
        &["preview", fresh.to_str().unwrap(), "--compare", "before"][..],
        &["preview", path, "--compare", &"0".repeat(64)][..],
    ] {
        let output = pentool(args, &[]);
        assert!(!output.status.success(), "{args:?}");
    }
}

#[test]
fn compare_in_a_focused_preview_uses_the_same_crop() {
    let space = Workspace::new("compare-focus");
    let doc = space.document();
    let result = run(&rect(&doc, "box", &["--compare", "before"]), &[]);
    let preview = &result["preview"];
    assert_eq!(preview["focus"]["ids"], serde_json::json!(["box"]));
    // Box (10,10)-(60,60) in a focus at (2,2), scale 8: pixels 64..464.
    let bbox = &preview["compare"]["changed_bbox"];
    assert_eq!(
        [&bbox["x"], &bbox["y"], &bbox["width"], &bbox["height"]],
        [64, 64, 400, 400]
    );
}

fn cache_entries(doc: &Path) -> Vec<PathBuf> {
    let root = doc.parent().unwrap().join(".pentool").join("cache");
    let mut entries: Vec<PathBuf> = fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|dir| fs::read_dir(dir.path()).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    entries
}

#[test]
fn render_cache_reuses_layers_between_calls_without_changing_bytes() {
    let space = Workspace::new("render-cache");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    let render = |name: &str, extra: &[&str], env: &[(&str, &str)]| {
        let out = space.0.join(name);
        json(&pentool(
            &[&["export", path, out.to_str().unwrap()], extra].concat(),
            env,
        ));
        fs::read(out).unwrap()
    };

    let uncached = render("a.png", &["--no-cache"], &[]);
    assert!(cache_entries(&doc).is_empty(), "--no-cache writes nothing");
    let off = render("b.png", &[], &[("PENTOOL_CACHE", "0")]);
    assert!(
        cache_entries(&doc).is_empty(),
        "PENTOOL_CACHE=0 writes nothing"
    );
    assert_eq!(off, uncached);

    assert_eq!(render("cold.png", &[], &[]), uncached);
    let entries = cache_entries(&doc);
    assert!(!entries.is_empty(), "a cached render keeps its layers");
    assert_eq!(render("warm.png", &[], &[]), uncached);
    assert_eq!(cache_entries(&doc), entries, "a warm render adds nothing");

    // Damaged entries are misses: they are replaced and the output is unchanged.
    for entry in &entries {
        let mut bytes = fs::read(entry).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x55;
        fs::write(entry, bytes).unwrap();
    }
    assert_eq!(render("damaged.png", &[], &[]), uncached);
    assert_eq!(render("repaired.png", &[], &[]), uncached);

    // An edit renders what changed; the rest comes from the cache.
    run(&rect(&doc, "badge", &[]), &[]);
    let edited = render("edited.png", &["--no-cache"], &[]);
    assert_ne!(edited, uncached);
    assert_eq!(render("edited-cached.png", &[], &[]), edited);
    assert!(cache_entries(&doc).len() > entries.len());

    // Previews share the cache and stay identical to the export.
    let preview = space.0.join("preview.png");
    let looked = json(&pentool(
        &["preview", path, "--out", preview.to_str().unwrap()],
        &[],
    ));
    let scale = looked["preview"]["scale"].as_f64().unwrap().to_string();
    let scaled = render("scaled.png", &["--scale", &scale, "--no-cache"], &[]);
    assert_eq!(fs::read(&preview).unwrap(), scaled);
}

#[test]
fn render_cache_follows_scale_page_size_fonts_and_linked_files() {
    let space = Workspace::new("render-cache-keys");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    let photo = space.0.join("photo.png");
    let pixels = |color: [u8; 4]| image::RgbaImage::from_pixel(40, 20, image::Rgba(color));
    pixels([200, 40, 40, 255]).save(&photo).unwrap();
    json(&pentool(
        &[
            "image",
            "add",
            path,
            "photo",
            "--file",
            "photo.png",
            "--layer",
            "content",
            "--x",
            "300",
            "--width",
            "120",
            "--height",
            "60",
            "--external",
        ],
        &[],
    ));
    json(&pentool(
        &[
            "text",
            path,
            "put",
            "title",
            "--layer",
            "content",
            "--content",
            "Cache",
            "--x",
            "40",
            "--y",
            "200",
            "--size",
            "48",
            "--font",
            "Brand Test",
        ],
        &[],
    ));
    let render = |name: &str, extra: &[&str], cached: bool| {
        let out = space.0.join(name);
        let args = [&["export", path, out.to_str().unwrap()], extra].concat();
        let args = [&args[..], if cached { &[] } else { &["--no-cache"][..] }].concat();
        let output = pentool(&args, &[]);
        (output.status.success(), fs::read(out).ok())
    };
    // Every cached render matches a render without the cache.
    let check = |step: &str, extra: &[&str]| {
        let warm_up = render(&format!("{step}-cold.png"), extra, true);
        let cached = render(&format!("{step}-warm.png"), extra, true);
        let fresh = render(&format!("{step}-fresh.png"), extra, false);
        assert!(fresh.0, "{step}: the uncached export failed");
        assert!(
            warm_up == fresh && cached == fresh,
            "{step} changed the bytes"
        );
        fresh.1.unwrap()
    };

    let base = check("base", &[]);
    assert!(!cache_entries(&doc).is_empty());
    assert_ne!(check("scale", &["--scale", "2"]), base);

    let mut value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let width = value["pages"][0]["canvas"]["width"].as_f64().unwrap();
    value["pages"][0]["canvas"]["width"] = (width + 64.0).into();
    fs::write(&doc, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let wider = check("page-size", &[]);
    assert_ne!(wider, base);

    json(&pentool(
        &[
            "font",
            path,
            "add",
            "Brand Test",
            "--file",
            "assets/fonts/AtkinsonHyperlegible-Bold.ttf",
        ],
        &[],
    ));
    // Document fonts are part of every cache key; text is never cached.
    check("font", &[]);

    // A changed linked file is read again, never served from the cache.
    pixels([40, 40, 200, 255]).save(&photo).unwrap();
    let relinked_cached = render("linked-warm.png", &[], true);
    let relinked_fresh = render("linked-fresh.png", &[], false);
    assert_eq!(relinked_cached, relinked_fresh);

    // An unusable cache directory is only a slower render.
    let cache = space.0.join(".pentool").join("cache");
    fs::remove_dir_all(&cache).unwrap();
    fs::write(&cache, b"not a directory").unwrap();
    let blocked = render("blocked.png", &[], true);
    assert_eq!(blocked, render("blocked-fresh.png", &[], false));
}

fn session(lines: &[String]) -> Vec<Value> {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["session", "--jsonl"])
        .env_remove("PENTOOL_PREVIEW")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn session_runs_commands_like_separate_processes() {
    let space = Workspace::new("session");
    let (one, many) = (space.0.join("one"), space.0.join("many"));
    fs::create_dir_all(&one).unwrap();
    fs::create_dir_all(&many).unwrap();
    let (single, shared) = (one.join("doc.pen"), many.join("doc.pen"));
    fs::copy("docs/fixtures/v4-scene.pen", &single).unwrap();
    fs::copy("docs/fixtures/v4-scene.pen", &shared).unwrap();
    let tree = |doc: &Path| vec!["tree".to_string(), doc.display().to_string()];

    let expected = [
        run(&tree(&single), &[]),
        run(&rect(&single, "badge", &[]), &[]),
        run(&tree(&single), &[]),
    ];
    let lines: Vec<String> = [tree(&shared), rect(&shared, "badge", &[]), tree(&shared)]
        .iter()
        .map(|args| serde_json::to_string(args).unwrap())
        .collect();
    let replies = session(&lines);
    assert_eq!(replies.len(), 3);
    // The two documents differ only in their directory.
    let without = |value: &Value, dir: &Path| {
        let native = serde_json::to_string(&dir.display().to_string()).unwrap();
        let slashed = dir
            .display()
            .to_string()
            .replace(std::path::MAIN_SEPARATOR, "/");
        value
            .to_string()
            .replace(native.trim_matches('"'), "DIR")
            .replace(&slashed, "DIR")
    };
    for (reply, expected) in replies.iter().zip(&expected) {
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(without(&reply["result"], &many), without(expected, &one));
    }
    assert_eq!(fs::read(&shared).unwrap(), fs::read(&single).unwrap());
}

#[test]
fn session_reports_bad_lines_and_keeps_going() {
    let space = Workspace::new("session-errors");
    let doc = space.document();
    let before = fs::read(&doc).unwrap();
    let path = doc.display().to_string();
    let guarded = rect(&doc, "badge", &["--if-revision", &"0".repeat(64)]);
    let replies = session(&[
        "not json".into(),
        r#"{"id": "x1", "args": ["no-such-command"]}"#.into(),
        serde_json::to_string(&["serve"]).unwrap(),
        serde_json::to_string(&guarded).unwrap(),
        serde_json::json!({"id": 2, "args": ["preview", path, "--max-size", "64"]}).to_string(),
    ]);
    assert_eq!(replies.len(), 5);
    for reply in &replies[..4] {
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(reply["error"]["message"].is_string());
    }
    assert_eq!(replies[1]["id"], "x1");
    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "a refused edit writes nothing"
    );
    assert_eq!(replies[4]["id"], 2);
    assert_eq!(replies[4]["ok"], true);
    let image = image::open(replies[4]["result"]["preview"]["path"].as_str().unwrap()).unwrap();
    assert_eq!(image.width().max(image.height()), 64);

    // Flags belong on each line, not on the session.
    let flagged = pentool(&["session", "--jsonl", "--preview"], &[]);
    assert!(!flagged.status.success());
    assert!(String::from_utf8_lossy(&flagged.stderr).contains("put them on each line"));
}

#[test]
fn thread_count_never_changes_rendered_bytes() {
    let space = Workspace::new("threads");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    // A full-page gradient fill and a blurred, translucent ellipse exercise the
    // parallel fill, blur and blend loops.
    let mut value: Value = serde_json::from_slice(&fs::read(&doc).unwrap()).unwrap();
    let nodes = value["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap();
    nodes.insert(
        0,
        serde_json::json!({"kind":"fill","id":"sky","x":0,"y":0,"width":800,"height":600,
            "fill":{"kind":"linear","x1":0,"y1":0,"x2":0,"y2":600,"stops":[
                {"position":0,"color":"#2f7fd6"},{"position":0.55,"color":"#6fb3ea"},
                {"position":1,"color":"#d4eefb"}]}}),
    );
    nodes.push(
        serde_json::json!({"kind":"ellipse","id":"glow","cx":300,"cy":200,
        "radius_x":170,"radius_y":120,"opacity":0.65,"style":{"fill":{"fallback":"#fff6d0"}},
        "effects":[{"id":"soft","kind":"blur","enabled":true,"opacity":1,
            "blend_mode":"normal","params":{"radius":40}}]}),
    );
    fs::write(&doc, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let render = |name: &str, threads: &str| {
        let out = space.0.join(name);
        json(&pentool(
            &[
                "export",
                path,
                out.to_str().unwrap(),
                "--scale",
                "2",
                "--no-cache",
            ],
            &[("PENTOOL_THREADS", threads)],
        ));
        fs::read(out).unwrap()
    };
    let one = render("one.png", "1");
    for threads in ["2", "3", "8"] {
        assert!(
            render(&format!("{threads}.png"), threads) == one,
            "{threads} threads changed the export"
        );
    }
}

#[test]
fn font_list_cache_never_changes_text_rendering() {
    let space = Workspace::new("font-cache");
    let doc = space.document();
    let path = doc.to_str().unwrap();
    json(&pentool(&["migrate", path, "--target", "6"], &[]));
    // The per-user cache folder, moved into the workspace.
    let home = space.0.join("home");
    fs::create_dir_all(&home).unwrap();
    let home = home.to_str().unwrap();
    let cached = if cfg!(windows) {
        Some(space.0.join("home").join("pentool").join("fonts.bin"))
    } else if cfg!(target_os = "macos") {
        Some(space.0.join("home/Library/Caches/pentool/fonts.bin"))
    } else {
        None
    };
    let export = |name: &str, extra: &[&str]| {
        let out = space.0.join(name);
        json(&pentool(
            &[&["export", path, out.to_str().unwrap()], extra].concat(),
            &[("LOCALAPPDATA", home), ("HOME", home)],
        ));
        fs::read(out).unwrap()
    };
    // `--no-cache` scans the installed fonts and writes nothing.
    let scanned = export("scanned.png", &["--no-cache"]);
    assert!(cached.as_ref().is_none_or(|file| !file.exists()));
    assert_eq!(export("cold.png", &[]), scanned);
    if let Some(file) = &cached {
        assert!(file.exists(), "a cached render keeps the font list");
        assert_eq!(export("warm.png", &[]), scanned);
        let mut bytes = fs::read(file).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0x55;
        fs::write(file, &bytes).unwrap();
        assert_eq!(export("damaged.png", &[]), scanned);
        assert_ne!(
            fs::read(file).unwrap(),
            bytes,
            "a damaged list is rewritten"
        );
        assert_eq!(export("repaired.png", &[]), scanned);
    }
}
