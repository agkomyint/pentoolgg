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

#[test]
fn shape_reports_every_missing_geometry_option_at_once() {
    // #222: ellipse reported --cx, then --cy, then --radius-x, one run at a time.
    let dir = Dir::new("ellipse");
    dir.ok(&["new", "a.pen"]);
    let message = dir.fail(&["shape", "a.pen", "ellipse", "e1", "--layer", "layer-1"]);
    for name in ["--cx", "--cy", "--radius-x", "--radius-y"] {
        assert!(message.contains(name), "{message}");
    }
}

#[test]
fn group_instantiate_honors_the_id_flag() {
    // #91: `group instantiate ... --id NEW` silently dropped --id.
    let dir = Dir::new("instid");
    dir.fixture("v4-scene.pen");
    dir.ok(&[
        "group",
        "v4-scene.pen",
        "promote",
        "card",
        "--component",
        "c",
    ]);
    dir.ok(&[
        "group",
        "v4-scene.pen",
        "instantiate",
        "card-copy",
        "--layer",
        "content",
        "--component",
        "c",
        "--id",
        "chosen",
    ]);
    let doc = std::fs::read_to_string(dir.0.join("v4-scene.pen")).unwrap();
    assert!(doc.contains("\"chosen\""), "{doc}");
    assert!(!doc.contains("\"card-copy\""), "{doc}");
}

#[test]
fn fonts_scan_ignores_adjustment_nodes() {
    let dir = Dir::new("fonts-adjustment");
    dir.ok(&["new", "r.pen"]);
    dir.ok(&["migrate", "r.pen", "--target", "6"]);
    dir.ok(&[
        "adjustment",
        "add",
        "r.pen",
        "ad1",
        "--kind",
        "brightness-contrast",
        "--params",
        r#"{"brightness":0.2,"contrast":0.2}"#,
    ]);
    dir.ok(&["fonts", "r.pen"]);
}

#[test]
fn instance_mutations_accept_and_enforce_if_revision() {
    let dir = Dir::new("instance-if-revision");
    dir.ok(&["new", "d.pen"]);
    let path = dir.0.join("d.pen");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["instances"] = serde_json::json!([{"id": "i1"}]);
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let err = dir.fail(&[
        "instance",
        "d.pen",
        "detach",
        "i1",
        "--if-revision",
        "sha256:stale",
    ]);
    assert!(err.contains("revision mismatch"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    for sub in ["update", "rollback", "set"] {
        let help = dir.ok(&["instance", "d.pen", sub, "--help"]);
        assert!(help.contains("--if-revision"), "{sub}: {help}");
    }
}

#[test]
fn add_accepts_an_explicit_asset_path() {
    let dir = Dir::new("add-by-path");
    dir.ok(&["new", "host.pen", "--width", "100", "--height", "100"]);
    dir.ok(&[
        "shape", "host.pen", "rect", "r", "--layer", "layer-1", "--x", "2", "--y", "2", "--width",
        "20", "--height", "20", "--fill", "#ff0000",
    ]);
    dir.ok(&[
        "asset", "create", "--id", "lib/r", "--name", "R", "host.pen", "r.pen",
    ]);
    dir.ok(&["new", "dest.pen", "--width", "100", "--height", "100"]);
    dir.ok(&["migrate", "dest.pen", "--target", "3"]);
    // r.pen is not in any registered library; the path alone must resolve.
    let out = dir.ok(&["add", "dest.pen", "r.pen", "--mode", "copy", "--dry-run"]);
    assert!(
        out.contains("\"dry_run\": true") || out.contains("dry_run"),
        "{out}"
    );
    dir.ok(&["add", "dest.pen", "r.pen", "--mode", "copy"]);
    let tree = dir.ok(&["tree", "dest.pen"]);
    assert!(tree.contains("lib-r") || tree.contains("r-"), "{tree}");
}

#[test]
fn package_verify_rejects_trailing_bytes_and_names_a_missing_file() {
    let dir = Dir::new("package-trailing");
    dir.ok(&["new", "h.pen", "--width", "50", "--height", "50"]);
    dir.ok(&[
        "shape", "h.pen", "rect", "r", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
        "5", "--height", "5", "--fill", "#ff0000",
    ]);
    dir.ok(&["package", "init", "kit", "--name", "kit"]);
    dir.ok(&[
        "asset",
        "create",
        "--id",
        "kit/r",
        "--name",
        "R",
        "h.pen",
        "kit/assets/r.pen",
    ]);
    dir.ok(&["package", "pack", "kit", "--output", "k.penpkg"]);
    dir.ok(&["package", "verify", "k.penpkg"]);
    let mut bytes = std::fs::read(dir.0.join("k.penpkg")).unwrap();
    bytes.push(b'x');
    std::fs::write(dir.0.join("k.penpkg"), bytes).unwrap();
    let err = dir.fail(&["package", "verify", "k.penpkg"]);
    assert!(
        err.contains("unexpected bytes after the ZIP end record"),
        "{err}"
    );
    let err = dir.fail(&["package", "verify", "missing.penpkg"]);
    assert!(err.contains("missing.penpkg"), "{err}");
}

#[test]
fn asset_preview_regenerates_a_corrupt_cache_entry() {
    let dir = Dir::new("asset-preview-cache");
    dir.ok(&["new", "h.pen", "--width", "50", "--height", "50"]);
    dir.ok(&[
        "shape", "h.pen", "rect", "r", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
        "5", "--height", "5", "--fill", "#ff0000",
    ]);
    dir.ok(&[
        "asset", "create", "--id", "a/r", "--name", "R", "h.pen", "r.pen",
    ]);
    let out = dir.ok(&["asset", "preview", "r.pen", "--output", "first.png"]);
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    let cache = dir.0.join(report["cache"].as_str().unwrap());
    let cache = if cache.exists() {
        cache
    } else {
        PathBuf::from(report["cache"].as_str().unwrap())
    };
    std::fs::write(&cache, b"garbage").unwrap();
    dir.ok(&["asset", "preview", "r.pen", "--output", "second.png"]);
    let png = std::fs::read(dir.0.join("second.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "{} bytes", png.len());
}

#[test]
fn library_refresh_skips_history_snapshots() {
    let dir = Dir::new("library-history");
    dir.ok(&["new", "h.pen", "--width", "50", "--height", "50"]);
    dir.ok(&[
        "shape", "h.pen", "rect", "r", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
        "5", "--height", "5", "--fill", "#ff0000",
    ]);
    std::fs::create_dir_all(dir.0.join("lib")).unwrap();
    dir.ok(&[
        "asset",
        "create",
        "--id",
        "a/r",
        "--name",
        "R",
        "h.pen",
        "lib/r.pen",
    ]);
    let snapshots = dir.0.join("lib/.pentool/history/r.pen");
    std::fs::create_dir_all(&snapshots).unwrap();
    std::fs::copy(dir.0.join("lib/r.pen"), snapshots.join("old.pen")).unwrap();
    dir.ok(&["library", "add", "--name", "l", "lib"]);
    let out = dir.ok(&["library", "refresh"]);
    assert!(!out.contains("duplicate_asset_id"), "{out}");
}

#[test]
fn instance_set_rejects_an_invalid_color_value() {
    let dir = Dir::new("instance-set-color");
    dir.ok(&["new", "h.pen", "--width", "50", "--height", "50"]);
    dir.ok(&[
        "shape", "h.pen", "rect", "nib", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
        "5", "--height", "5", "--fill", "#ff0000",
    ]);
    dir.ok(&[
        "asset",
        "create",
        "--id",
        "brand/logo",
        "--name",
        "L",
        "h.pen",
        "logo.pen",
    ]);
    dir.ok(&[
        "asset",
        "property",
        "add",
        "logo.pen",
        "brand/logo",
        "accent",
        "--target",
        "nib",
        "--field",
        "fill",
        "--default",
        "#ff0000",
    ]);
    dir.ok(&["new", "d.pen", "--width", "50", "--height", "50"]);
    dir.ok(&["migrate", "d.pen", "--target", "3"]);
    dir.ok(&["add", "d.pen", "logo.pen", "--mode", "instance"]);
    let before = std::fs::read(dir.0.join("d.pen")).unwrap();
    let err = dir.fail(&[
        "instance",
        "d.pen",
        "set",
        "instance-1",
        "--property",
        "accent",
        "--value",
        "notacolor",
        "--source",
        "logo.pen",
    ]);
    assert!(err.contains("not a color"), "{err}");
    assert_eq!(std::fs::read(dir.0.join("d.pen")).unwrap(), before);
    dir.ok(&[
        "instance",
        "d.pen",
        "set",
        "instance-1",
        "--property",
        "accent",
        "--value",
        "#00ff00",
        "--source",
        "logo.pen",
    ]);
}

#[test]
fn preset_and_fill_help_state_the_v6_prerequisite() {
    let dir = Dir::new("help-v6");
    let help = dir.ok(&["--help"]);
    for command in ["fill", "preset"] {
        let line = help
            .lines()
            .find(|l| l.trim_start().starts_with(command) && l.contains("v6"))
            .unwrap_or_default();
        assert!(!line.is_empty(), "{command}: {help}");
    }
}

#[test]
fn photo_info_reports_a_rendered_source_instead_of_tiff_error() {
    let dir = Dir::new("photo-info-rendered");
    dir.ok(&["new", "p.pen", "--width", "40", "--height", "30"]);
    dir.ok(&["migrate", "p.pen", "--target", "6"]);
    let pixels =
        image::RgbImage::from_fn(8, 6, |x, y| image::Rgb([x as u8 * 30, y as u8 * 40, 90]));
    let mut jpeg = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let path = dir.0.join("p.pen");
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    pentool::photo::catalog::add_rendered(
        &mut raw,
        "beach",
        "beach.jpg",
        jpeg.get_ref(),
        pentool::image::embedded_storage(jpeg.get_ref()),
    )
    .unwrap();
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    let out = dir.ok(&["photo", "info", "p.pen", "beach"]);
    assert!(out.contains("rendered"), "{out}");
    assert!(!out.contains("TIFF"), "{out}");
}

#[test]
fn image_add_external_accepts_an_absolute_path_inside_the_document_directory() {
    let dir = Dir::new("image-external-abs");
    dir.ok(&["new", "a.pen", "--width", "40", "--height", "30"]);
    image::RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 255]))
        .save(dir.0.join("magenta.png"))
        .unwrap();
    let inside = dir.0.join("magenta.png");
    dir.ok(&[
        "image",
        "add",
        "a.pen",
        "pic",
        "--file",
        inside.to_str().unwrap(),
        "--layer",
        "layer-1",
        "--width",
        "16",
        "--height",
        "16",
        "--external",
    ]);
    let doc = std::fs::read_to_string(dir.0.join("a.pen")).unwrap();
    assert!(doc.contains("\"path\": \"magenta.png\""), "{doc}");
    let outside = std::env::temp_dir().join(format!("pentool-outside-{}.png", std::process::id()));
    image::RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 0]))
        .save(&outside)
        .unwrap();
    let err = dir.fail(&[
        "image",
        "add",
        "a.pen",
        "pic2",
        "--file",
        outside.to_str().unwrap(),
        "--layer",
        "layer-1",
        "--width",
        "16",
        "--height",
        "16",
        "--external",
    ]);
    let _ = std::fs::remove_file(&outside);
    assert!(err.contains("inside the document directory"), "{err}");
}

#[test]
fn bare_missing_file_errors_carry_a_code_and_a_hint() {
    let dir = Dir::new("missing-file");
    let err = dir.fail(&["info", "NOPE.pen"]);
    assert!(
        err.contains("[missing-resource]") && err.contains("check that the path"),
        "{err}"
    );
    std::fs::create_dir(dir.0.join("empty")).unwrap();
    let err = dir.fail(&["extension", "pack", "empty", "out.penext"]);
    assert!(err.contains("[missing-resource]"), "{err}");
}

#[test]
fn info_revision_and_history_expose_the_current_revision() {
    let dir = Dir::new("current-revision");
    dir.ok(&["new", "r.pen"]);
    dir.ok(&["page", "r.pen", "add", "p2"]);
    let info: serde_json::Value =
        serde_json::from_str(&dir.ok(&["info", "r.pen", "--revision"])).unwrap();
    let revision = info["revision"].as_str().unwrap().to_owned();
    assert_eq!(info["history_depth"], 1);
    let history: serde_json::Value = serde_json::from_str(&dir.ok(&["history", "r.pen"])).unwrap();
    assert_eq!(history["revision"], revision);
    dir.ok(&["undo", "r.pen"]);
    let history: serde_json::Value = serde_json::from_str(&dir.ok(&["history", "r.pen"])).unwrap();
    assert_ne!(history["revision"], revision);
    dir.ok(&[
        "migrate",
        "r.pen",
        "--target",
        "6",
        "--if-revision",
        history["revision"].as_str().unwrap(),
    ]);
}

#[test]
fn studio_scene_operations_use_the_type_key() {
    fn scan(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan(&path, hits);
            } else if path.extension().is_some_and(|e| e == "ts" || e == "tsx") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("{ kind: \"mask-") {
                    hits.push(path.display().to_string());
                }
            }
        }
    }
    let mut hits = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("web-ui/src"),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "/api/scene operations key on `type`, not `kind`: {hits:?}"
    );
}

#[test]
fn stroke_path_of_one_degenerate_segment_is_an_empty_success() {
    let dir = Dir::new("degenerate-stroke");
    dir.ok(&["new", "d.pen"]);
    dir.ok(&["migrate", "d.pen", "--target", "6"]);
    dir.ok(&[
        "raster", "d.pen", "add", "rasterA", "--width", "64", "--height", "64",
    ]);
    let before = std::fs::read(dir.0.join("d.pen")).unwrap();
    let out = dir.ok(&[
        "raster",
        "d.pen",
        "stroke-path",
        "rasterA",
        "--path",
        "M 10 10 L 10 10",
        "--json",
    ]);
    assert!(out.contains("\"dabs\""), "{out}");
    let _ = before;
}

#[test]
fn asset_property_set_updates_default_without_retargeting() {
    let dir = Dir::new("property-set-default");
    dir.ok(&["new", "h.pen", "--width", "50", "--height", "50"]);
    dir.ok(&[
        "shape", "h.pen", "rect", "nib", "--layer", "layer-1", "--x", "1", "--y", "1", "--width",
        "5", "--height", "5", "--fill", "#ff0000",
    ]);
    dir.ok(&[
        "asset",
        "create",
        "--id",
        "brand/logo",
        "--name",
        "L",
        "h.pen",
        "logo.pen",
    ]);
    dir.ok(&[
        "asset",
        "property",
        "add",
        "logo.pen",
        "brand/logo",
        "accent",
        "--target",
        "nib",
        "--field",
        "fill",
        "--default",
        "#ff0000",
    ]);
    dir.ok(&[
        "asset",
        "property",
        "set",
        "logo.pen",
        "brand/logo",
        "accent",
        "--default",
        "#00ff00",
        "--label",
        "Accent",
    ]);
    let doc = std::fs::read_to_string(dir.0.join("logo.pen")).unwrap();
    assert!(
        doc.contains("#00ff00") && doc.contains("Accent") && doc.contains("\"nib\""),
        "{doc}"
    );
    let err = dir.fail(&[
        "asset",
        "property",
        "set",
        "logo.pen",
        "brand/logo",
        "accent",
    ]);
    assert!(err.contains("--default or --label"), "{err}");
}

#[test]
fn batch_accepts_inline_json_stdin_and_at_file() {
    let dir = Dir::new("batch-inline");
    dir.ok(&["new", "b.pen"]);
    let op = |id: &str| {
        format!(
            r##"[{{"type":"put-shape","id":"{id}","layer":"layer-1","shape":"rect","x":1,"y":1,"width":5,"height":5,"fill":"#ff0000"}}]"##
        )
    };
    dir.ok(&["batch", "b.pen", &op("inline")]);
    std::fs::write(dir.0.join("ops.json"), op("fromfile")).unwrap();
    dir.ok(&["batch", "b.pen", "@ops.json"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&dir.0)
        .args(["batch", "b.pen", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), op("piped").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = std::fs::read_to_string(dir.0.join("b.pen")).unwrap();
    for id in ["inline", "fromfile", "piped"] {
        assert!(doc.contains(id), "{id} missing: {doc}");
    }
}

#[test]
fn export_dash_streams_the_rendered_bytes_to_stdout() {
    let dir = Dir::new("export-stdout");
    dir.ok(&["new", "e.pen", "--width", "20", "--height", "10"]);
    let out = dir.run(&["export", "e.pen", "-", "--format", "svg"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .trim_start()
            .starts_with("<svg"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let png = dir.run(&["export", "e.pen", "-", "--format", "png"]);
    assert_eq!(&png.stdout[..4], b"\x89PNG");
    let err = dir.fail(&["export", "e.pen", "-"]);
    assert!(err.contains("--format"), "{err}");
}

#[test]
fn new_takes_background_name_and_a_revision_guard_for_overwrite() {
    let dir = Dir::new("new-options");
    dir.ok(&["new", "n.pen", "--background", "#112233", "--name", "Brand"]);
    let info = dir.ok(&["info", "n.pen"]);
    assert!(
        info.contains("#112233") && info.contains("\"Brand\""),
        "{info}"
    );
    let err = dir.fail(&["new", "n.pen", "--background", "nope", "--overwrite"]);
    assert!(err.contains("not a color"), "{err}");
    let before = std::fs::read(dir.0.join("n.pen")).unwrap();
    let err = dir.fail(&[
        "new",
        "n.pen",
        "--overwrite",
        "--if-revision",
        &"0".repeat(64),
    ]);
    assert!(err.to_lowercase().contains("revision"), "{err}");
    assert_eq!(std::fs::read(dir.0.join("n.pen")).unwrap(), before);
    let revision: serde_json::Value =
        serde_json::from_str(&dir.ok(&["info", "n.pen", "--revision"])).unwrap();
    dir.ok(&[
        "new",
        "n.pen",
        "--overwrite",
        "--if-revision",
        revision["revision"].as_str().unwrap(),
    ]);
}
