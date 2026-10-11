//! Version 8 migration, profile identity, offline storage and atomic CLI edits.
use pentool::{color::catalog, composite, photo, resource, scene, transaction};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const CMYK: &[u8] = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}

#[test]
fn normative_catalog_fixture_matches_its_pinned_manifest() {
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    let bytes = include_bytes!("../docs/fixtures/print-v8.pen");
    assert_eq!(
        format!(
            "sha256:{}",
            manifest["documents"][0]["sha256"].as_str().unwrap()
        ),
        resource::sha256(bytes)
    );
    scene::validate(&fixture()).unwrap();
    let schema: Value =
        serde_json::from_slice(include_bytes!("../docs/pen-format-v8.schema.json")).unwrap();
    assert_eq!(schema["properties"]["version"]["const"], 8);
}

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pentool-color-catalog-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn explicit_migration_preserves_legacy_scene_and_photography_and_refuses_conflicts() {
    let mut raw = scene::new_document(64, 48);
    raw["custom"] = json!({"keep":[1,2,3]});
    raw["pages"][0]["layers"][0]["locked"] = json!(true);
    let v7 = photo::catalog::migrate(raw.clone()).unwrap();
    let v8 = catalog::migrate(raw).unwrap();
    assert_eq!(v8["version"], 8);
    assert_eq!(v8["color_management"], catalog::empty());
    assert_eq!(catalog::downgrade(v8.clone()).unwrap(), v7);
    assert_eq!(catalog::migrate(v8.clone()).unwrap(), v8);
    let photo: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/photo-v7.pen")).unwrap();
    let migrated = catalog::migrate(photo.clone()).unwrap();
    assert_eq!(migrated["photography"], photo["photography"]);
    assert_eq!(migrated["pages"], photo["pages"]);
    assert_eq!(photo::catalog::migrate(migrated).unwrap(), photo);
    let mut extension = scene::new_document(10, 10);
    extension["color_management"] = json!({"user_extension":true});
    scene::validate(&extension).unwrap();
    assert!(catalog::migrate(extension)
        .unwrap_err()
        .to_string()
        .contains("extension conflicts"));
    let mut future = v8;
    future["version"] = json!(9);
    assert!(transaction::validate_value(&future).is_err());
}

#[test]
fn imports_are_content_addressed_unassigned_and_fail_atomically_on_conflicts() {
    let mut raw = fixture();
    let first = catalog::import(
        &mut raw,
        Path::new("art.pen"),
        RGB,
        "display",
        "unknown",
        Some("authored probe"),
        None,
    )
    .unwrap();
    let identity = first["profile"].as_str().unwrap();
    assert_eq!(identity, resource::sha256(RGB));
    assert_eq!(first["assigned"], false);
    assert_eq!(
        catalog::load(&raw, Path::new("art.pen"), "profile:display")
            .unwrap()
            .1,
        RGB
    );
    let before = raw.clone();
    for (bytes, alias, policy, source) in [
        (CMYK, "display", "unknown", Some("authored probe")),
        (RGB, "second", "allowed", Some("authored probe")),
        (RGB, "bad/name", "unknown", None),
        (&b"invalid"[..], "bad", "unknown", None),
    ] {
        assert!(catalog::import(
            &mut raw,
            Path::new("art.pen"),
            bytes,
            alias,
            policy,
            source,
            None
        )
        .is_err());
        assert_eq!(raw, before);
    }
    let again = catalog::import(
        &mut raw,
        Path::new("art.pen"),
        RGB,
        "alternate",
        "unknown",
        Some("authored probe"),
        None,
    )
    .unwrap();
    assert_eq!(again["reused"], true);
    assert_eq!(
        raw["color_management"]["profiles"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(catalog::list(&raw, 0, 1).unwrap()["returned"], 1);
    assert_eq!(
        catalog::list(&raw, usize::MAX, 1).unwrap()["has_more"],
        false
    );
    for target in [3, 4, 5, 6, 7] {
        let result = match target {
            3 => scene::flatten_to_v3(&raw),
            4 => scene::migrate_to_v4(raw.clone()),
            5 => scene::migrate_to_v5(raw.clone()),
            6 => composite::migrate(raw.clone()),
            _ => photo::catalog::migrate(raw.clone()),
        };
        assert!(result.is_err(), "target {target}");
        assert_eq!(
            raw["color_management"]["profiles"][identity],
            before["color_management"]["profiles"][identity]
        );
    }
    let mut old = photo::catalog::migrate(fixture()).unwrap();
    let before = old.clone();
    assert!(catalog::import(
        &mut old,
        Path::new("art.pen"),
        RGB,
        "display",
        "unknown",
        None,
        None
    )
    .is_err());
    assert_eq!(old, before);
}

#[test]
fn original_bytes_header_metadata_and_lengths_are_all_verified() {
    let mut raw = fixture();
    catalog::import(
        &mut raw,
        Path::new("art.pen"),
        RGB,
        "test",
        "unknown",
        None,
        None,
    )
    .unwrap();
    let identity = resource::sha256(RGB);
    for (key, value) in [
        ("byte_length", json!(16777217)),
        ("icc_version", json!("4.2.0")),
        ("class", json!("prtr")),
        ("color_space", json!("CMYK")),
        ("metadata", json!({})),
        ("redistribution", json!("guess")),
    ] {
        let mut bad = raw.clone();
        bad["color_management"]["profiles"][&identity][key] = value;
        assert!(scene::validate(&bad).is_err(), "{key}");
    }
    let mut corrupt = raw.clone();
    let data = corrupt["color_management"]["profiles"][&identity]["storage"]["data"]
        .as_str()
        .unwrap();
    let changed = format!("!{}", &data[1..]);
    corrupt["color_management"]["profiles"][&identity]["storage"]["data"] = json!(changed);
    assert!(scene::validate(&corrupt).is_err());
    let mut missing = raw.clone();
    missing["color_management"]["aliases"]["absent"] = json!(format!("sha256:{}", "0".repeat(64)));
    assert!(scene::validate(&missing).is_err());
    assert!(catalog::load(&raw, Path::new("art.pen"), "absent").is_err());
    let record = raw["color_management"]["profiles"][&identity].clone();
    let mut many = fixture();
    for i in 0..65 {
        many["color_management"]["profiles"][format!("sha256:{i:064x}")] = record.clone();
    }
    assert!(format!("{:#}", scene::validate(&many).unwrap_err()).contains("64 profiles"));
    let mut total = fixture();
    let mut external = record;
    external["byte_length"] = json!(16777216);
    external["storage"] = json!({"kind":"external","path":"probe.icc"});
    for i in 0..5 {
        total["color_management"]["profiles"][format!("sha256:{i:064x}")] = external.clone();
    }
    assert!(format!("{:#}", scene::validate(&total).unwrap_err()).contains("decoded bytes"));
}

#[test]
fn external_profiles_embed_offline_and_missing_cache_reads_obey_icc_limits() {
    let work = Workspace::new("external");
    let document = work.file("art.pen");
    std::fs::write(work.file("profile.icc"), RGB).unwrap();
    let mut raw = fixture();
    catalog::import(
        &mut raw,
        &document,
        RGB,
        "test",
        "allowed",
        None,
        Some("profile.icc"),
    )
    .unwrap();
    let before = raw.clone();
    for unsafe_path in ["../profile.icc", "C:/profile.icc", "assets\\profile.icc"] {
        assert!(catalog::import(
            &mut raw,
            &document,
            RGB,
            "unsafe",
            "allowed",
            None,
            Some(unsafe_path)
        )
        .is_err());
        assert_eq!(raw, before);
    }
    std::fs::write(work.file("profile.icc"), CMYK).unwrap();
    assert!(catalog::embed(&mut raw, &document, "test").is_err());
    assert_eq!(raw, before);
    std::fs::remove_file(work.file("profile.icc")).unwrap();
    assert!(catalog::load(&raw, &document, "test").is_err());
    let cached = resource::cache_store(&work.0, RGB).unwrap();
    assert_eq!(catalog::load(&raw, &document, "test").unwrap().1, RGB);
    assert!(catalog::import(
        &mut raw,
        &document,
        RGB,
        "missing-external",
        "allowed",
        None,
        Some("missing.icc"),
    )
    .is_err());
    assert_eq!(raw, before);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&cached)
        .unwrap();
    file.set_len(16777217).unwrap();
    drop(file);
    assert!(
        format!("{:#}", catalog::load(&raw, &document, "test").unwrap_err())
            .contains("limit-exceeded")
    );
    std::fs::write(&cached, RGB).unwrap();
    catalog::embed(&mut raw, &document, "test").unwrap();
    assert_eq!(
        raw["color_management"]["profiles"][resource::sha256(RGB)]["storage"]["kind"],
        "embedded"
    );
    assert_eq!(catalog::load(&raw, &document, "test").unwrap().1, RGB);
}

#[test]
fn migrated_and_unused_profile_scenes_render_identically_and_keep_photo_edits_at_v8() {
    let raw = fixture();
    let v7 = catalog::downgrade(raw.clone()).unwrap();
    let path = Path::new("target/print-render.pen");
    let expected = composite::render(&v7, path, None, 1.0).unwrap();
    let mut with_profile = raw;
    catalog::import(
        &mut with_profile,
        path,
        RGB,
        "unused",
        "unknown",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        composite::render(&with_profile, path, None, 1.0)
            .unwrap()
            .as_raw(),
        expected.as_raw()
    );
    assert_eq!(
        composite::png(&with_profile, path, None, 1.0).unwrap(),
        composite::png(&v7, path, None, 1.0).unwrap()
    );
    let color = with_profile["color_management"].clone();
    let lens = json!({"pentool_lens_profile":1,"make":"Pentool","model":"Test","focal_range":[35,35],"aperture_range":[2,2],"samples":[{"focal":35,"aperture":2,"distortion":{"radial":[0,0,0]}}]});
    photo::catalog::add_lens_profile(&mut with_profile, &serde_json::to_vec(&lens).unwrap())
        .unwrap();
    assert_eq!(with_profile["version"], 8);
    assert_eq!(with_profile["color_management"], color);
    scene::validate(&with_profile).unwrap();
}

#[test]
fn v8_catalog_documents_keep_discovery_and_standard_exports_compatible() {
    let work = Workspace::new("exports");
    let mut raw = fixture();
    catalog::import(
        &mut raw,
        &work.file("art.pen"),
        RGB,
        "unused",
        "unknown",
        None,
        None,
    )
    .unwrap();
    std::fs::write(work.file("art.pen"), serde_json::to_vec(&raw).unwrap()).unwrap();
    let before = std::fs::read(work.file("art.pen")).unwrap();
    let tree = work.ok(&["tree", "art.pen", "--limit", "1", "--json"]);
    assert_eq!(tree["returned"], 1);
    let search = work.ok(&["search", "art.pen", "color-card", "--limit", "1", "--json"]);
    assert_eq!(search["matches"], 1);
    assert_eq!(search["returned"], 1);
    for output in ["art.png", "art.svg", "art.pdf"] {
        let result = work.run(&["export", "art.pen", output]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let png = image::load_from_memory(&std::fs::read(work.file("art.png")).unwrap())
        .unwrap()
        .to_rgba8();
    assert_eq!(png.dimensions(), (64, 48));
    assert_eq!(png.get_pixel(32, 24).0, [36, 104, 172, 255]);
    assert_eq!(png.get_pixel(0, 0).0, [255, 255, 255, 255]);
    assert!(std::fs::read_to_string(work.file("art.svg"))
        .unwrap()
        .contains("<svg"));
    assert!(std::fs::read(work.file("art.pdf"))
        .unwrap()
        .starts_with(b"%PDF-"));
    assert_eq!(std::fs::read(work.file("art.pen")).unwrap(), before);
    assert!(!work.file(".pentool/history").exists());
}

#[test]
fn cli_migration_and_import_use_one_transaction_and_failed_guards_preserve_bytes() {
    let work = Workspace::new("cli");
    let doc = work.file("art.pen");
    std::fs::write(
        &doc,
        serde_json::to_vec(&scene::new_document(64, 48)).unwrap(),
    )
    .unwrap();
    std::fs::write(work.file("profile.icc"), RGB).unwrap();
    let before = std::fs::read(&doc).unwrap();
    work.ok(&["migrate", "art.pen", "--target", "8", "--dry-run", "--json"]);
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    work.ok(&["migrate", "art.pen", "--target", "8", "--json"]);
    let before = std::fs::read(&doc).unwrap();
    let history = pentool::history::list(&doc).unwrap();
    let dry = work.ok(&[
        "color",
        "profile",
        "import",
        "art.pen",
        "profile.icc",
        "--name",
        "test",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    assert_eq!(pentool::history::list(&doc).unwrap(), history);
    let failed = work.run(&[
        "color",
        "profile",
        "import",
        "art.pen",
        "profile.icc",
        "--name",
        "test",
        "--if-revision",
        "wrong",
        "--json",
    ]);
    assert!(!failed.status.success());
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    assert_eq!(pentool::history::list(&doc).unwrap(), history);
    let committed = work.ok(&[
        "color",
        "profile",
        "import",
        "art.pen",
        "profile.icc",
        "--name",
        "test",
        "--json",
    ]);
    assert_eq!(dry["result"], committed["result"]);
    assert_eq!(
        pentool::history::list(&doc).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        history["entries"].as_array().unwrap().len() + 1
    );
    let shown = work.ok(&[
        "color", "profile", "show", "art.pen", "--name", "test", "--limit", "1", "--json",
    ]);
    assert_eq!(shown["profile"], resource::sha256(RGB));
    assert_eq!(shown["inspection"]["returned"], 1);
    let listed = work.ok(&[
        "color", "profile", "list", "art.pen", "--limit", "1", "--json",
    ]);
    assert_eq!(listed["returned"], 1);
    let before = std::fs::read(&doc).unwrap();
    for target in ["3", "4", "5", "6", "7"] {
        assert!(!work
            .run(&["migrate", "art.pen", "--target", target, "--json"])
            .status
            .success());
        assert_eq!(std::fs::read(&doc).unwrap(), before);
    }
    work.ok(&["undo", "art.pen", "--json"]);
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        serde_json::to_vec_pretty(&catalog::migrate(scene::new_document(64, 48)).unwrap()).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn document_and_cache_symlinks_cannot_escape_the_project() {
    let work = Workspace::new("symlink");
    let outside = Workspace::new("outside");
    std::fs::write(outside.file("profile.icc"), RGB).unwrap();
    std::os::unix::fs::symlink(outside.file("profile.icc"), work.file("escape.icc")).unwrap();
    assert!(resource::read_external_offline_limited(
        &work.file("art.pen"),
        Path::new("escape.icc"),
        &resource::sha256(RGB),
        16777216
    )
    .is_err());
    let cache = resource::cache_dir(&work.0);
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(
        outside.file("profile.icc"),
        cache.join(resource::sha256(RGB).trim_start_matches("sha256:")),
    )
    .unwrap();
    assert!(resource::cache_read_limited(&work.0, &resource::sha256(RGB), 16777216).is_err());
    assert!(resource::cache_store(&work.0, RGB).is_err());
    let second = Workspace::new("cache-parent-link");
    std::os::unix::fs::symlink(&outside.0, second.file(".pentool")).unwrap();
    assert!(resource::cache_store(&second.0, RGB).is_err());
    assert_eq!(std::fs::read(outside.file("profile.icc")).unwrap(), RGB);
    assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 1);
}

#[test]
fn private_cache_writers_are_idempotent_under_concurrency_and_leave_no_temps() {
    let work = Workspace::new("cache-concurrent");
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| resource::cache_store(&work.0, RGB).unwrap()))
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
    });
    let files: Vec<_> = std::fs::read_dir(resource::cache_dir(&work.0))
        .unwrap()
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        resource::cache_read(&work.0, &resource::sha256(RGB))
            .unwrap()
            .unwrap(),
        RGB
    );
}

#[test]
fn cli_external_import_and_embed_have_identical_dry_run_plans_and_revision_guards() {
    let work = Workspace::new("cli-embed");
    let document = work.file("art.pen");
    std::fs::write(&document, serde_json::to_vec_pretty(&fixture()).unwrap()).unwrap();
    std::fs::write(work.file("profile.icc"), RGB).unwrap();
    work.ok(&[
        "color",
        "profile",
        "import",
        "art.pen",
        "profile.icc",
        "--name",
        "press",
        "--external",
        "profile.icc",
        "--json",
    ]);
    let before = std::fs::read(&document).unwrap();
    let dry = work.ok(&[
        "color",
        "profile",
        "embed",
        "art.pen",
        "--name",
        "press",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(std::fs::read(&document).unwrap(), before);
    assert!(!work
        .run(&[
            "color",
            "profile",
            "embed",
            "art.pen",
            "--name",
            "press",
            "--if-revision",
            "wrong",
            "--json"
        ])
        .status
        .success());
    assert_eq!(std::fs::read(&document).unwrap(), before);
    let actual = work.ok(&[
        "color", "profile", "embed", "art.pen", "--name", "press", "--json",
    ]);
    assert_eq!(actual["result"], dry["result"]);
    assert_eq!(std::fs::read(work.file("profile.icc")).unwrap(), RGB);
    std::fs::remove_file(work.file("profile.icc")).unwrap();
    work.ok(&[
        "color", "profile", "show", "art.pen", "--name", "press", "--json",
    ]);
}
