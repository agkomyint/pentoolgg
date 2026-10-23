//! Regression tests for stabilization fixes that are CLI-visible and need no helpers.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pentool-stab-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn fail(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(!out.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn fixture(&self, name: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/fixtures")
            .join(name);
        let target = self.0.join(name);
        std::fs::copy(source, &target).unwrap();
        target
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn locked_layers_reject_shape_and_path_writes() {
    // #16: shape and path put used to bypass a locked layer.
    let dir = Dir::new("locked");
    dir.fixture("v4-scene.pen");
    dir.ok(&[
        "layer",
        "v4-scene.pen",
        "set",
        "content",
        "--locked",
        "true",
    ]);
    let before = std::fs::read(dir.0.join("v4-scene.pen")).unwrap();
    let shape = dir.fail(&[
        "shape",
        "v4-scene.pen",
        "rect",
        "zz",
        "--layer",
        "content",
        "--x",
        "0",
        "--y",
        "0",
        "--width",
        "5",
        "--height",
        "5",
        "--fill",
        "#000000",
    ]);
    assert!(shape.contains("locked"), "{shape}");
    let path = dir.fail(&[
        "path",
        "v4-scene.pen",
        "put",
        "--layer",
        "content",
        "--d",
        "M0 0L5 5",
        "pz",
    ]);
    assert!(path.contains("locked"), "{path}");
    assert_eq!(before, std::fs::read(dir.0.join("v4-scene.pen")).unwrap());
}

#[test]
fn path_put_refuses_to_replace_a_group() {
    // #17: `path put` over a group ID destroyed the group and its children.
    let dir = Dir::new("pathgroup");
    dir.fixture("v4-scene.pen");
    let before = std::fs::read(dir.0.join("v4-scene.pen")).unwrap();
    let message = dir.fail(&[
        "path",
        "v4-scene.pen",
        "put",
        "--layer",
        "content",
        "--d",
        "M0 0L10 10",
        "card",
    ]);
    assert!(message.contains("not a path"), "{message}");
    assert_eq!(before, std::fs::read(dir.0.join("v4-scene.pen")).unwrap());
}

#[test]
fn pdf_export_honors_page_and_rejects_unknown_pages() {
    // #11: PDF export ignored --page and always wrote every page.
    let dir = Dir::new("pdfpage");
    dir.ok(&["new", "a.pen"]);
    dir.ok(&["page", "a.pen", "add", "second"]);
    dir.ok(&["export", "a.pen", "all.pdf"]);
    dir.ok(&["export", "a.pen", "one.pdf", "--page", "second"]);
    let count = |name: &str| {
        let text = String::from_utf8_lossy(&std::fs::read(dir.0.join(name)).unwrap()).into_owned();
        let tail = &text[text.find("/Count ").unwrap() + 7..];
        tail[..tail.find(' ').unwrap()].parse::<usize>().unwrap()
    };
    let (all, one) = (count("all.pdf"), count("one.pdf"));
    assert!(all > one, "all pages {all}, one page {one}");
    let message = dir.fail(&["export", "a.pen", "bogus.pdf", "--page", "bogus"]);
    assert!(message.contains("page not found"), "{message}");
}

#[test]
fn negative_numbers_reach_value_validation_in_nested_commands() {
    // #61: `text put --size -5` died in clap instead of reporting the range.
    let dir = Dir::new("negative");
    dir.ok(&["new", "a.pen"]);
    let message = dir.fail(&[
        "text",
        "a.pen",
        "put",
        "t1",
        "--layer",
        "layer-1",
        "--content",
        "hi",
        "--size",
        "-5",
    ]);
    assert!(!message.contains("unexpected argument"), "{message}");
}

#[test]
fn group_instantiate_rejects_ignored_id_flag() {
    // #91: `group instantiate ... --id NEW` silently dropped --id.
    let dir = Dir::new("instid");
    dir.fixture("v4-scene.pen");
    let message = dir.fail(&[
        "group",
        "v4-scene.pen",
        "instantiate",
        "card",
        "--layer",
        "content",
        "--component",
        "c",
        "--id",
        "other",
    ]);
    assert!(message.contains("--id"), "{message}");
}

#[test]
fn selection_query_rejects_duplicate_keys() {
    // #75: duplicate JSON keys silently collapsed (last wins).
    let dir = Dir::new("dupkeys");
    dir.ok(&["new", "a.pen"]);
    let message = dir.fail(&[
        "selection",
        "query",
        "a.pen",
        "--query",
        r#"{"kind":"rectangle","x":0,"y":0,"width":5,"height":5,"width":10}"#,
    ]);
    assert!(message.contains("duplicate key"), "{message}");
}

#[test]
fn library_writes_leave_no_numbered_backups() {
    // #87: every library write used to leave a new `.bak.N` file in `.pentool`.
    let dir = Dir::new("libbak");
    std::fs::create_dir_all(dir.0.join("lib")).unwrap();
    dir.ok(&["library", "add", "--name", "bak", "lib"]);
    for _ in 0..3 {
        dir.ok(&["library", "refresh"]);
        dir.ok(&["library", "disable", "bak"]);
        dir.ok(&["library", "enable", "bak"]);
    }
    let backups: Vec<_> = std::fs::read_dir(dir.0.join(".pentool"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
        .collect();
    assert!(backups.is_empty(), "{backups:?}");
}

#[test]
fn geometry_help_names_the_legacy_only_subcommands() {
    // #88: help advertised subcommands that v4 scenes reject without saying so.
    let dir = Dir::new("geohelp");
    let help = dir.ok(&["geometry", "--help"]);
    assert!(
        help.matches("legacy v1-v3 documents only").count() >= 5,
        "{help}"
    );
}

#[test]
fn paint_field_width_is_limited_to_4096_cells() {
    // #137: only the total-cell bound was enforced, not 4,096 cells per dimension.
    let dir = Dir::new("fielddim");
    dir.ok(&["new", "d.pen"]);
    dir.ok(&[
        "raster", "d.pen", "add", "dimL", "--width", "16384", "--height", "64",
    ]);
    let message = dir.fail(&[
        "paint-fields",
        "d.pen",
        "preview",
        "dimL",
        "--field",
        r#"{"method":"angle","degrees":45,"cell":1,"rect":[0,0,16384,64]}"#,
        "--max-vectors",
        "0",
        "--json",
    ]);
    assert!(message.contains("limit-exceeded"), "{message}");
}

#[test]
fn negative_zero_field_params_hash_like_zero() {
    // #136: `-0` and `0` produced different field hashes.
    let dir = Dir::new("negzero");
    dir.ok(&["new", "d.pen"]);
    dir.ok(&[
        "raster", "d.pen", "add", "rasterA", "--width", "64", "--height", "64",
    ]);
    let hash = |field: &str| {
        let out = dir.ok(&[
            "paint-fields",
            "d.pen",
            "preview",
            "rasterA",
            "--field",
            field,
            "--json",
        ]);
        let start = out.find("sha256:").unwrap();
        out[start..start + 71].to_owned()
    };
    assert_eq!(
        hash(r#"{"method":"angle","degrees":0}"#),
        hash(r#"{"method":"angle","degrees":-0.0}"#)
    );
}

#[test]
fn link_images_errors_when_nothing_can_be_linked() {
    // #12: `--link-images` on a document without images silently did nothing.
    let dir = Dir::new("linkimg");
    dir.fixture("v5-image-embedded.pen");
    let message = dir.fail(&["export", "v5-image-embedded.pen", "a.svg", "--link-images"]);
    assert!(message.contains("--link-images"), "{message}");
}

#[test]
fn preview_honors_the_global_preview_out_flag() {
    // #219: `preview DOC --preview-out PATH` wrote to .pentool/preview instead.
    let dir = Dir::new("previewout");
    dir.ok(&["new", "doc.pen"]);
    dir.ok(&["preview", "doc.pen", "--preview-out", "requested.png"]);
    assert!(dir.0.join("requested.png").exists());
}

#[test]
fn library_doctor_reports_a_missing_enabled_library_folder() {
    // #233: doctor said ok:true while an enabled library folder no longer existed.
    let dir = Dir::new("doctor");
    std::fs::create_dir_all(dir.0.join("lib")).unwrap();
    dir.ok(&["library", "add", "--name", "gone", "lib"]);
    dir.ok(&["library", "refresh"]);
    std::fs::remove_dir_all(dir.0.join("lib")).unwrap();
    let report = dir.ok(&["library", "doctor"]);
    assert!(report.contains("missing_folder"), "{report}");
    assert!(report.contains("\"ok\": false"), "{report}");
}
