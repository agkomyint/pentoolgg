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
    let result = run(&rect(&doc, "box", &["--preview"]), &[]);
    let preview = &result["preview"];
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
        &rect(&doc, "box", &["--preview-size", "200"]),
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
