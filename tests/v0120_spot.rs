//! v8 spot ink catalog validation and offline package dependency collection.
use pentool::{
    color::spot::{self, Ink, Library, Provenance},
    package, scene,
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

fn document() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}

fn ink(id: &str, redistribution: &str) -> Ink {
    Ink {
        id: id.into(),
        name: "Studio Orange".into(),
        aliases: vec!["brand-orange".into()],
        alternate: json!({"space":"srgb","channels":[0.95,0.22,0.03]}),
        provenance: Provenance {
            source: "local ink library".into(),
            publisher: Some("Studio".into()),
            revision: Some("2026-01".into()),
            license: Some("internal".into()),
        },
        redistribution: redistribution.into(),
    }
}

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pentool-spot-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn package_asset(work: &Workspace, raw: &Value) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = work.path(&format!(
        "kit-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    package::init(&dir, "spot-kit").unwrap();
    let mut raw = raw.clone();
    raw["asset"] = json!({"schema":1,"id":"spot-card","name":"Spot Card"});
    fs::write(
        dir.join("assets/card.pen"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    dir
}

#[test]
fn v8_catalog_accepts_optional_ink_library_and_rejects_alias_conflicts() {
    let mut raw = document();
    scene::validate(&raw).unwrap(); // old v8 documents without inks remain valid

    let mut library = Library::default();
    library.insert(ink("studio-orange", "allowed")).unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    scene::validate(&raw).unwrap();
    assert_eq!(
        spot::document_library(&raw)
            .unwrap()
            .resolve("BRAND-ORANGE")
            .unwrap()
            .id,
        "studio-orange"
    );

    let mut conflicting = library.clone();
    let mut duplicate = ink("second-orange", "allowed");
    duplicate.name = "Another Orange".into();
    conflicting.inks.insert(duplicate.id.clone(), duplicate);
    raw["color_management"]["inks"] = serde_json::to_value(conflicting).unwrap();
    assert!(scene::validate(&raw).is_err());
}

#[test]
fn spot_alternates_must_reference_a_matching_catalog_profile() {
    let mut raw = document();
    let mut library = Library::default();
    let mut value = ink("studio-orange", "allowed");
    value.alternate = json!({"space":"icc-rgb","profile":"missing","channels":[0.95,0.22,0.03]});
    library.insert(value).unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(library).unwrap();
    let error = scene::validate(&raw).unwrap_err().to_string();
    assert!(
        error.contains("alternate references missing ICC profile"),
        "{error}"
    );
}

#[test]
fn offline_package_collects_and_verifies_spot_definitions_deterministically() {
    let work = Workspace::new();
    let mut raw = document();
    let mut library = Library::default();
    library.insert(ink("studio-orange", "allowed")).unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    let dir = package_asset(&work, &raw);
    let first = work.path("first.penpkg");
    let second = work.path("second.penpkg");
    package::pack(&dir, &first).unwrap();
    package::pack(&dir, &second).unwrap();
    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

    let verified = package::verify(&first).unwrap();
    let table = verified["package"]["assets"]["spot-card"]["spot_inks"]
        .as_array()
        .unwrap();
    assert_eq!(table.len(), 1);
    assert_eq!(table[0]["ink"]["id"], "studio-orange");
    assert_eq!(table[0]["ink"]["provenance"]["source"], "local ink library");

    let mut restricted = raw;
    restricted["color_management"]["inks"]["inks"]["studio-orange"]["redistribution"] =
        json!("restricted");
    let restricted_dir = package_asset(&work, &restricted);
    let keep = work.path("keep.penpkg");
    fs::write(&keep, b"existing package").unwrap();
    assert!(package::pack(&restricted_dir, &keep).is_err());
    assert_eq!(fs::read(&keep).unwrap(), b"existing package");
}

#[test]
fn manifest_spot_dependency_table_is_recomputed_from_the_asset() {
    let work = Workspace::new();
    let mut raw = document();
    let mut library = Library::default();
    library.insert(ink("studio-orange", "allowed")).unwrap();
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    let dir = package_asset(&work, &raw);
    let archive = work.path("spot.penpkg");
    package::pack(&dir, &archive).unwrap();
    let report = package::verify(&archive).unwrap();
    let collected = report["package"]["assets"]["spot-card"]["spot_inks"].clone();
    assert_eq!(
        collected,
        serde_json::to_value(spot::package_table(&raw).unwrap()).unwrap()
    );

    let forged = work.path("forged.penpkg");
    let mut zip = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut manifest_bytes = Vec::new();
    use std::io::Read;
    zip.by_name("pentool.package.json")
        .unwrap()
        .read_to_end(&mut manifest_bytes)
        .unwrap();
    let mut manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    manifest["assets"]["spot-card"]["spot_inks"][0]["ink"]["name"] = json!("Forged");
    let mut payload = Vec::new();
    zip.by_name("assets/card.pen")
        .unwrap()
        .read_to_end(&mut payload)
        .unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(&forged).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    use std::io::Write;
    writer.start_file("pentool.package.json", options).unwrap();
    writer
        .write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    writer.start_file("assets/card.pen", options).unwrap();
    writer.write_all(&payload).unwrap();
    writer.finish().unwrap();
    assert!(package::verify(&forged).is_err());
}
