//! ICC collection, explicit unused-profile cleanup and offline package auditing.
use base64::Engine;
use pentool::{color::catalog, composite, package, resource, scene};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const CMYK: &[u8] = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");
fn doc() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pentool-icc-collection-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
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
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn external(raw: &mut Value, work: &Workspace, bytes: &[u8], name: &str, policy: &str) {
    let file = format!("{name}.icc");
    fs::write(work.path(&file), bytes).unwrap();
    catalog::import(
        raw,
        &work.path("art.pen"),
        bytes,
        name,
        policy,
        Some("original fixture"),
        Some(&file),
    )
    .unwrap();
}

#[test]
fn collection_verifies_every_dependency_before_replacing_the_document() {
    let work = Workspace::new();
    let path = work.path("art.pen");
    let mut raw = doc();
    external(&mut raw, &work, RGB, "rgb", "allowed");
    external(&mut raw, &work, CMYK, "press", "allowed");
    let original = raw.clone();
    fs::write(work.path("press.icc"), b"corrupt").unwrap();
    assert!(catalog::collect(&mut raw, &path, false).is_err());
    assert_eq!(raw, original);
    fs::write(work.path("press.icc"), CMYK).unwrap();
    let report = catalog::collect(&mut raw, &path, false).unwrap();
    assert_eq!(report["matched"], 2);
    assert_eq!(report["embedded"], 2);
    assert_eq!(report["byte_length"], RGB.len() + CMYK.len());
    for bytes in [RGB, CMYK] {
        let identity = resource::sha256(bytes);
        assert_eq!(catalog::load(&raw, &path, &identity).unwrap().1, bytes);
        let mut expected = original["color_management"]["profiles"][&identity].clone();
        expected["storage"] = raw["color_management"]["profiles"][&identity]["storage"].clone();
        assert_eq!(expected, raw["color_management"]["profiles"][&identity]);
    }
    fs::remove_file(work.path("press.icc")).unwrap();
    fs::remove_file(work.path("rgb.icc")).unwrap();
    assert_eq!(
        catalog::collect(&mut raw, &path, false).unwrap()["already_embedded"],
        2
    );
    assert_eq!(
        catalog::package_table(&raw).unwrap(),
        report["profiles"].as_array().unwrap().clone()
    );
}

#[test]
fn restricted_and_unknown_policies_require_explicit_local_collection_and_still_block_packages() {
    for policy in ["restricted", "unknown"] {
        let work = Workspace::new();
        let mut raw = doc();
        external(&mut raw, &work, RGB, "rgb", policy);
        let before = raw.clone();
        fs::remove_file(work.path("rgb.icc")).unwrap();
        assert!(format!(
            "{:#}",
            catalog::collect(&mut raw, &work.path("art.pen"), false).unwrap_err()
        )
        .contains("redistribution-restricted"));
        assert_eq!(raw, before);
        fs::write(work.path("rgb.icc"), RGB).unwrap();
        assert_eq!(
            catalog::collect(&mut raw, &work.path("art.pen"), true).unwrap()["local_only"],
            true
        );
        assert_eq!(
            raw["color_management"]["profiles"][resource::sha256(RGB)]["redistribution"],
            policy
        );
        assert!(format!("{:#}", catalog::package_table(&raw).unwrap_err())
            .contains("redistribution-restricted"));
    }
}

#[test]
fn unused_cleanup_removes_all_aliases_but_never_discards_referenced_native_paints() {
    let work = Workspace::new();
    let mut raw = doc();
    external(&mut raw, &work, RGB, "rgb", "allowed");
    let identity = resource::sha256(RGB);
    raw["color_management"]["aliases"]["second"] = json!(identity);
    raw["styles"] = json!({"unused":{"type":"color","value":{"space":"icc-rgb","profile":"second","channels":[0.1,0.2,0.3]}}});
    let before = raw.clone();
    assert!(
        format!("{:#}", catalog::remove(&mut raw, "rgb").unwrap_err()).contains("resource-in-use")
    );
    assert_eq!(raw, before);
    raw["styles"] = json!({});
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"second","channels":[0.1,0.2,0.3]});
    pentool::mask::create(
        &mut raw,
        &work.path("art.pen"),
        None,
        "snapshot",
        "vector:color-card",
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] = json!("#336699");
    let before = raw.clone();
    assert!(catalog::remove(&mut raw, &identity).is_err());
    assert_eq!(raw, before);
    raw.as_object_mut().unwrap().remove("masks");
    raw.as_object_mut().unwrap().remove("mask_resources");
    fs::remove_file(work.path("rgb.icc")).unwrap();
    assert!(catalog::remove(&mut raw, "absent").is_err());
    let report = catalog::remove(&mut raw, "profile:second").unwrap();
    assert_eq!(report["aliases"], json!(["rgb", "second"]));
    assert_eq!(raw["color_management"], catalog::empty());
    catalog::downgrade(raw).unwrap();
}

#[test]
fn cli_collection_and_cleanup_use_identical_dry_run_plans_guards_and_undo() {
    let work = Workspace::new();
    let mut raw = doc();
    external(&mut raw, &work, RGB, "rgb", "allowed");
    let original = serde_json::to_vec(&raw).unwrap();
    fs::write(work.path("art.pen"), &original).unwrap();
    let dry = work.ok(&[
        "color",
        "profile",
        "collect",
        "art.pen",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), original);
    assert!(!work
        .run(&[
            "color",
            "profile",
            "collect",
            "art.pen",
            "--if-revision",
            "stale"
        ])
        .status
        .success());
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), original);
    let committed = work.ok(&["color", "profile", "collect", "art.pen", "--json"]);
    assert_eq!(dry["result"], committed["result"]);
    assert_eq!(
        dry["change"]["revision_after"],
        committed["change"]["revision_after"]
    );
    assert_eq!(
        pentool::history::list(&work.path("art.pen")).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(work.path("rgb.icc").exists());
    work.ok(&["undo", "art.pen", "--json"]);
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), original);
    let dry = work.ok(&[
        "color",
        "profile",
        "remove",
        "art.pen",
        "--name",
        "rgb",
        "--dry-run",
        "--json",
    ]);
    let committed = work.ok(&[
        "color", "profile", "remove", "art.pen", "--name", "rgb", "--json",
    ]);
    assert_eq!(dry["result"], committed["result"]);
    work.ok(&["undo", "art.pen", "--json"]);
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), original);
    assert!(!work
        .run(&["color", "profile", "remove", "art.pen", "--name", "missing"])
        .status
        .success());
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), original);
    assert!(!work
        .run(&["--page", "page-1", "color", "profile", "collect", "art.pen"])
        .status
        .success());
}

fn package_asset(work: &Workspace, raw: &Value) -> PathBuf {
    let dir = work.path("kit");
    package::init(&dir, "print-kit").unwrap();
    let mut raw = raw.clone();
    raw["asset"] = json!({"schema":1,"id":"print-card","name":"Print Card"});
    fs::write(
        dir.join("assets/card.pen"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    dir
}
fn forge(work: &Workspace, manifest: &Value, payload: &[u8]) -> PathBuf {
    let path = work.path("forged.penpkg");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("pentool.package.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    writer.start_file("assets/card.pen", options).unwrap();
    writer.write_all(payload).unwrap();
    writer.finish().unwrap();
    path
}

#[test]
fn packages_pin_original_icc_bytes_audit_tables_and_reproduce_offline() {
    let work = Workspace::new();
    let mut raw = doc();
    external(&mut raw, &work, RGB, "rgb", "allowed");
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"rgb","channels":[0.2,0.4,0.6]});
    let before = composite::render(&raw, &work.path("art.pen"), None, 1.0).unwrap();
    catalog::collect(&mut raw, &work.path("art.pen"), false).unwrap();
    let dir = package_asset(&work, &raw);
    let first = work.path("first.penpkg");
    let second = work.path("second.penpkg");
    package::pack(&dir, &first).unwrap();
    package::pack(&dir, &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    let verified = package::verify(&first).unwrap();
    assert_eq!(
        verified["package"]["assets"]["print-card"]["icc_profiles"],
        serde_json::to_value(catalog::package_table(&raw).unwrap()).unwrap()
    );
    let mut archive = zip::ZipArchive::new(fs::File::open(&first).unwrap()).unwrap();
    let mut payload = Vec::new();
    archive
        .by_name("assets/card.pen")
        .unwrap()
        .read_to_end(&mut payload)
        .unwrap();
    let packed: Value = serde_json::from_slice(&payload).unwrap();
    let data = packed["color_management"]["profiles"][resource::sha256(RGB)]["storage"]["data"]
        .as_str()
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap(),
        RGB
    );
    fs::remove_file(work.path("rgb.icc")).unwrap();
    assert_eq!(
        before.as_raw(),
        composite::render(&packed, Path::new("offline/card.pen"), None, 1.0)
            .unwrap()
            .as_raw()
    );
    let mut manifest = verified["package"].clone();
    manifest["assets"]["print-card"]["icc_profiles"][0]["byte_length"] = json!(1);
    assert!(format!(
        "{:#}",
        package::verify(&forge(&work, &manifest, &payload)).unwrap_err()
    )
    .contains("ICC profile table"));
    manifest["assets"]["print-card"]
        .as_object_mut()
        .unwrap()
        .remove("icc_profiles");
    assert!(package::verify(&forge(&work, &manifest, &payload)).is_err());
    scene::validate(&packed).unwrap();
}

#[test]
fn packages_refuse_external_unlicensed_and_corrupt_profiles_without_overwriting_outputs() {
    for policy in ["allowed", "unknown", "restricted"] {
        let work = Workspace::new();
        let mut raw = doc();
        external(&mut raw, &work, RGB, "rgb", policy);
        let dir = package_asset(&work, &raw);
        let out = work.path("keep.penpkg");
        fs::write(&out, b"existing output").unwrap();
        assert!(package::pack(&dir, &out).is_err());
        assert_eq!(fs::read(&out).unwrap(), b"existing output");
        catalog::collect(&mut raw, &work.path("art.pen"), true).unwrap();
        raw["asset"] = json!({"schema":1,"id":"print-card","name":"Print Card"});
        if policy == "allowed" {
            let data = raw["color_management"]["profiles"][resource::sha256(RGB)]["storage"]
                ["data"]
                .as_str()
                .unwrap()
                .to_owned();
            raw["color_management"]["profiles"][resource::sha256(RGB)]["storage"]["data"] =
                json!(format!("!{}", &data[1..]));
        }
        fs::write(
            dir.join("assets/card.pen"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        assert!(package::pack(&dir, &out).is_err());
        assert_eq!(fs::read(&out).unwrap(), b"existing output");
    }
}

#[test]
fn manifest_only_assets_cannot_bypass_icc_licensing_checks() {
    let work = Workspace::new();
    let dir = work.path("manual-kit");
    package::init(&dir, "manual-kit").unwrap();
    let mut raw = doc();
    catalog::import(
        &mut raw,
        &work.path("art.pen"),
        RGB,
        "rgb",
        "unknown",
        None,
        None,
    )
    .unwrap();
    raw["asset"] = json!({"schema":1,"id":"manual-card","name":"Manual Card"});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(dir.join("loose.pen"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("pentool.package.json")).unwrap()).unwrap();
    manifest["assets"] =
        json!({"manual-card":{"path":"loose.pen","hash":resource::sha256(&bytes)}});
    fs::write(
        dir.join("pentool.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let output = work.path("keep.penpkg");
    fs::write(&output, b"keep").unwrap();
    assert!(package::pack(&dir, &output).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"keep");
}

#[test]
fn package_previews_cannot_replace_a_validated_asset_or_manifest() {
    let work = Workspace::new();
    let dir = work.path("manual-kit");
    package::init(&dir, "manual-kit").unwrap();
    let mut raw = doc();
    catalog::import(
        &mut raw,
        &work.path("art.pen"),
        RGB,
        "rgb",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["asset"] = json!({"schema":1,"id":"manual-card","name":"Manual Card"});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(dir.join("loose.pen"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("pentool.package.json")).unwrap()).unwrap();
    manifest["assets"] =
        json!({"manual-card":{"path":"loose.pen","hash":resource::sha256(&bytes)}});
    fs::write(
        dir.join("pentool.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let good = work.path("manual.penpkg");
    package::pack(&dir, &good).unwrap();
    package::verify(&good).unwrap();
    for preview in ["pentool.package.json", "loose.pen"] {
        manifest["assets"]["manual-card"]["preview"] = json!(preview);
        fs::write(
            dir.join("pentool.package.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let output = work.path("keep.penpkg");
        fs::write(&output, b"keep").unwrap();
        assert!(
            package::pack(&dir, &output).is_err(),
            "preview collision {preview}"
        );
        assert_eq!(fs::read(&output).unwrap(), b"keep");
    }
}

#[test]
fn edited_package_assets_do_not_redistribute_hidden_history_snapshots() {
    let work = Workspace::new();
    let dir = package_asset(&work, &doc());
    let document = dir.join("assets/card.pen");
    fs::write(work.path("rgb.icc"), RGB).unwrap();
    work.ok(&[
        "color",
        "profile",
        "import",
        document.to_str().unwrap(),
        "rgb.icc",
        "--name",
        "rgb",
        "--redistribution",
        "allowed",
        "--json",
    ]);
    assert_eq!(
        pentool::history::list(&document).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let out = work.path("edited.penpkg");
    package::pack(&dir, &out).unwrap();
    let verified = package::verify(&out).unwrap();
    assert_eq!(verified["package"]["assets"].as_object().unwrap().len(), 1);
    let mut archive = zip::ZipArchive::new(fs::File::open(out).unwrap()).unwrap();
    for index in 0..archive.len() {
        assert!(!archive.by_index(index).unwrap().name().contains(".pentool"));
    }
    let private_dir = dir.join(".pentool");
    fs::create_dir(&private_dir).unwrap();
    let mut private = doc();
    private["asset"] = json!({"schema":1,"id":"private-card","name":"Private Card"});
    let bytes = serde_json::to_vec(&private).unwrap();
    fs::write(private_dir.join("private.pen"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(dir.join("pentool.package.json")).unwrap()).unwrap();
    manifest["assets"]["private-card"] =
        json!({"path":".pentool/private.pen","hash":resource::sha256(&bytes)});
    fs::write(
        dir.join("pentool.package.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(format!(
        "{:#}",
        package::pack(&dir, &work.path("private.penpkg")).unwrap_err()
    )
    .contains("private-resource"));
}
