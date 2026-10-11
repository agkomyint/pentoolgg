//! Explicit role settings, executable intent paths and transactional consumers.
use pentool::{
    color::{
        catalog,
        icc::Intent,
        roles::{self, Role},
        value,
    },
    composite, package, resource, scene,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const CMYK: &[u8] = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");
const PRESS: &[u8] = include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc");
fn doc() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
fn sample() -> Value {
    json!({"space":"srgb","channels":[0.2,0.4,0.7],"alpha":0.4})
}
fn dest(space: &str) -> Value {
    json!({"space":space})
}
fn icc(space: &str, profile: &str) -> Value {
    json!({"space":space,"profile":profile})
}
fn import(raw: &mut Value, bytes: &[u8], name: &str) {
    catalog::import(
        raw,
        Path::new("art.pen"),
        bytes,
        name,
        "allowed",
        None,
        None,
    )
    .unwrap();
}

#[test]
fn normative_roles_fixture_is_pinned_offline_and_preserves_preview_pixels() {
    let bytes = include_bytes!("../docs/fixtures/print-v8-roles.pen");
    let raw: Value = serde_json::from_slice(bytes).unwrap();
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    let entry = manifest["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file"] == "print-v8-roles.pen")
        .unwrap();
    assert_eq!(
        format!("sha256:{}", entry["sha256"].as_str().unwrap()),
        resource::sha256(bytes)
    );
    scene::validate(&raw).unwrap();
    let before = raw.clone();
    let report = roles::list(&raw).unwrap();
    assert_eq!(report["roles"][0]["configured"], true);
    assert_eq!(report["roles"][1]["configured"], false);
    assert_eq!(report["roles"][2]["configured"], true);
    assert_eq!(report["roles"][3]["configured"], true);
    let (identity, _) = catalog::resolve(&raw, "rgb-probe").unwrap();
    assert_eq!(report["roles"][2]["profile"], identity);
    assert!(roles::effective(&raw, Role::Proof).is_err());
    let base: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8-native.pen")).unwrap();
    let path = Path::new("target/offline-role-fixture.pen");
    assert_eq!(
        composite::render(&raw, path, None, 1.0).unwrap(),
        composite::render(&base, path, None, 1.0).unwrap()
    );
    assert_eq!(raw, before);
}
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pentool-color-roles-{}-{}-{}",
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

#[test]
fn defaults_are_explicitly_reported_and_unconfigured_conditions_never_fall_back() {
    let raw = doc();
    let report = roles::list(&raw).unwrap();
    assert_eq!(report["matches"], 4);
    assert_eq!(report["returned"], 4);
    assert_eq!(report["has_more"], false);
    assert_eq!(
        report["roles"][0]["effective"]["destination"]["space"],
        "srgb:linear"
    );
    assert_eq!(
        report["roles"][2]["effective"]["destination"]["space"],
        "srgb"
    );
    for role in [Role::Proof, Role::Output] {
        assert!(roles::effective(&raw, role).is_err());
        assert!(roles::evaluate(&raw, Path::new("art.pen"), role, &sample()).is_err());
    }
    assert!("input".parse::<Role>().is_err());
    let old = pentool::photo::catalog::migrate(raw).unwrap();
    assert!(roles::list(&old).is_err());
}

#[test]
fn assignment_changes_only_settings_and_sample_conversion_consumes_the_stored_intent() {
    let mut raw = doc();
    raw["extension"] = json!({"keep":[1,2,3]});
    raw["pages"][0]["layers"][0]["locked"] = json!(true);
    let original = raw.clone();
    let path = Path::new("target/role-assignment.pen");
    let before = composite::render(&raw, path, None, 1.0).unwrap();
    let report = roles::assign(
        &mut raw,
        path,
        Role::Working,
        &dest("display-p3"),
        Intent::Saturation,
    )
    .unwrap();
    assert_eq!(report["paint_samples_changed"], false);
    assert_eq!(report["validated_paths"].as_array().unwrap().len(), 2);
    assert_eq!(raw["pages"], original["pages"]);
    assert_eq!(raw["extension"], original["extension"]);
    assert_eq!(
        before.as_raw(),
        composite::render(&raw, path, None, 1.0).unwrap().as_raw()
    );
    let converted = roles::evaluate(&raw, path, Role::Working, &sample()).unwrap();
    let expected = value::evaluate(
        &raw,
        path,
        &sample(),
        Some(&dest("display-p3")),
        Intent::Saturation,
    )
    .unwrap();
    assert_eq!(converted["converted"], expected["converted"]);
    assert_eq!(converted["intent"], "saturation");
    assert_eq!(converted["mutated"], false);
    assert!(catalog::downgrade(raw.clone()).is_err());
    roles::clear(&mut raw, Role::Working).unwrap();
    assert_eq!(raw, original);
    assert!(roles::clear(&mut raw, Role::Working).is_err());
    catalog::downgrade(raw).unwrap();
}

#[test]
fn icc_roles_pin_identity_enforce_channel_and_class_contracts_and_block_removal() {
    let mut raw = doc();
    import(&mut raw, RGB, "rgb");
    let path = Path::new("art.pen");
    let rgb = icc("icc-rgb", "rgb");
    roles::assign(
        &mut raw,
        path,
        Role::Display,
        &rgb,
        Intent::RelativeColorimetric,
    )
    .unwrap();
    assert_eq!(
        raw["color_management"]["roles"]["display"]["destination"]["profile"],
        resource::sha256(RGB)
    );
    let before = raw.clone();
    for (role, target) in [
        (Role::Display, dest("lab-d50")),
        (Role::Display, icc("icc-gray", "rgb")),
        (Role::Proof, dest("srgb")),
        (Role::Proof, rgb.clone()),
        (Role::Output, rgb.clone()),
        (Role::Output, dest("lab-d50")),
    ] {
        assert!(
            roles::assign(&mut raw, path, role, &target, Intent::RelativeColorimetric).is_err()
        );
        assert_eq!(raw, before);
    }
    assert!(
        format!("{:#}", catalog::remove(&mut raw, "rgb").unwrap_err()).contains("display role")
    );
    assert_eq!(raw, before);
    raw["color_management"]["aliases"]
        .as_object_mut()
        .unwrap()
        .remove("rgb");
    scene::validate(&raw).unwrap();
    assert_eq!(
        roles::list(&raw).unwrap()["roles"][2]["profile"],
        resource::sha256(RGB)
    );
    roles::clear(&mut raw, Role::Display).unwrap();
    catalog::remove(&mut raw, &resource::sha256(RGB)).unwrap();
    let mut link = RGB.to_vec();
    link[12..16].copy_from_slice(b"link");
    link[20..24].copy_from_slice(b"RGB ");
    import(&mut raw, &link, "link");
    assert!(format!(
        "{:#}",
        roles::assign(
            &mut raw,
            path,
            Role::Working,
            &icc("icc-rgb", "link"),
            Intent::RelativeColorimetric
        )
        .unwrap_err()
    )
    .contains("device-link"));
}

#[test]
fn real_press_role_conversions_match_the_shared_all_intent_engine_without_mutating_master() {
    let mut raw = doc();
    import(&mut raw, PRESS, "press");
    let path = Path::new("art.pen");
    let target = icc("icc-cmyk", "press");
    for intent in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ] {
        let report = roles::assign(&mut raw, path, Role::Proof, &target, intent).unwrap();
        assert_eq!(report["validated_paths"].as_array().unwrap().len(), 2);
        roles::assign(&mut raw, path, Role::Output, &target, intent).unwrap();
        let before = raw.clone();
        let canonical = roles::effective(&raw, Role::Proof)
            .unwrap()
            .destination
            .json();
        let expected = value::evaluate(&raw, path, &sample(), Some(&canonical), intent).unwrap();
        for role in [Role::Proof, Role::Output] {
            let actual = roles::evaluate(&raw, path, role, &sample()).unwrap();
            assert_eq!(actual["converted"], expected["converted"]);
            assert_eq!(actual["source"], sample());
        }
        assert_eq!(raw, before);
    }
}

#[test]
fn unsupported_paths_and_malformed_settings_fail_before_mutation_or_external_loading() {
    let mut raw = doc();
    import(&mut raw, CMYK, "one-way");
    let before = raw.clone();
    let target = icc("icc-cmyk", "one-way");
    for role in [Role::Working, Role::Proof, Role::Output] {
        assert!(roles::assign(
            &mut raw,
            Path::new("art.pen"),
            role,
            &target,
            Intent::RelativeColorimetric
        )
        .is_err());
        assert_eq!(raw, before);
    }
    let mut manual = raw.clone();
    let mut printer = CMYK.to_vec();
    printer[12..16].copy_from_slice(b"prtr");
    import(&mut manual, &printer, "one-way-print");
    manual["color_management"]["roles"] =
        json!({"proof":{"destination":icc("icc-cmyk","one-way-print"),"intent":"perceptual"}});
    scene::validate(&manual).unwrap();
    assert!(catalog::package_table(&manual).is_err());
    for roles in [
        json!([]),
        json!({"unknown":{"destination":{"space":"srgb"},"intent":"relative-colorimetric"}}),
        json!({"working":{"destination":{"space":"srgb"},"intent":"unknown"}}),
        json!({"working":{"destination":{"space":"srgb"},"intent":"perceptual","ignored":true}}),
        json!({"display":{"destination":{"space":"icc-rgb","profile":"absent"},"intent":"relative-colorimetric"}}),
    ] {
        let mut invalid = raw.clone();
        invalid["color_management"]["roles"] = roles;
        assert!(scene::validate(&invalid).is_err());
    }
}

#[test]
fn external_role_structure_stays_offline_but_assignment_and_consumers_verify_required_bytes() {
    let work = Workspace::new();
    let path = work.path("art.pen");
    fs::write(work.path("rgb.icc"), RGB).unwrap();
    let mut raw = doc();
    catalog::import(
        &mut raw,
        &path,
        RGB,
        "rgb",
        "allowed",
        None,
        Some("rgb.icc"),
    )
    .unwrap();
    roles::assign(
        &mut raw,
        &path,
        Role::Display,
        &icc("icc-rgb", "rgb"),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    fs::remove_file(work.path("rgb.icc")).unwrap();
    scene::validate(&raw).unwrap();
    roles::list(&raw).unwrap();
    composite::render(&raw, &path, None, 1.0).unwrap();
    roles::evaluate(&raw, &path, Role::Working, &sample()).unwrap();
    assert!(roles::evaluate(&raw, &path, Role::Display, &sample()).is_err());
    let before = raw.clone();
    assert!(roles::assign(
        &mut raw,
        &path,
        Role::Working,
        &icc("icc-rgb", "rgb"),
        Intent::RelativeColorimetric
    )
    .is_err());
    assert_eq!(raw, before);
    roles::clear(&mut raw, Role::Display).unwrap();
    catalog::remove(&mut raw, "rgb").unwrap();
}

#[test]
fn cli_roles_use_revision_guarded_history_and_role_paint_conversion_uses_the_same_setting() {
    let work = Workspace::new();
    let bytes = serde_json::to_vec(&doc()).unwrap();
    fs::write(work.path("art.pen"), &bytes).unwrap();
    let args = [
        "color",
        "role",
        "assign",
        "art.pen",
        "working",
        "--to",
        r#"{"space":"display-p3"}"#,
        "--intent",
        "saturation",
        "--json",
    ];
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let dry = work.ok(&dry);
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), bytes);
    let mut stale = args.to_vec();
    stale.extend(["--if-revision", "stale"]);
    assert!(!work.run(&stale).status.success());
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), bytes);
    let committed = work.ok(&args);
    assert_eq!(committed["result"], dry["result"]);
    assert_eq!(
        committed["change"]["revision_after"],
        dry["change"]["revision_after"]
    );
    let after = fs::read(work.path("art.pen")).unwrap();
    let sample_json = serde_json::to_string(&sample()).unwrap();
    let evaluated = work.ok(&[
        "color",
        "role",
        "evaluate",
        "art.pen",
        "working",
        "--color",
        &sample_json,
        "--json",
    ]);
    assert_eq!(evaluated["intent"], "saturation");
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), after);
    let converted = work.ok(&[
        "color",
        "paints",
        "convert",
        "art.pen",
        "--role",
        "working",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(converted["result"]["intent"], "saturation");
    assert_eq!(converted["result"]["target"]["space"], "display-p3");
    for extra in [
        vec!["--intent", "saturation"],
        vec!["--to", r#"{"space":"srgb"}"#],
        vec!["--legacy-srgb"],
    ] {
        let mut args = vec!["color", "paints", "convert", "art.pen", "--role", "working"];
        args.extend(extra);
        assert!(!work.run(&args).status.success());
    }
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), after);
    assert_eq!(
        pentool::history::list(&work.path("art.pen")).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    work.ok(&["undo", "art.pen", "--json"]);
    assert_eq!(fs::read(work.path("art.pen")).unwrap(), bytes);
    assert!(!work
        .run(&["--page", "missing", "color", "role", "list", "art.pen"])
        .status
        .success());
    let fallback = work.ok(&[
        "color",
        "paints",
        "convert",
        "art.pen",
        "--role",
        "working",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(fallback["result"]["target"]["space"], "srgb:linear");
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    work.ok(&dry);
}

#[test]
fn role_profile_dependencies_survive_collection_and_package_verification_offline() {
    let work = Workspace::new();
    let path = work.path("art.pen");
    fs::write(work.path("rgb.icc"), RGB).unwrap();
    let mut raw = doc();
    catalog::import(
        &mut raw,
        &path,
        RGB,
        "rgb",
        "allowed",
        None,
        Some("rgb.icc"),
    )
    .unwrap();
    roles::assign(
        &mut raw,
        &path,
        Role::Working,
        &icc("icc-rgb", "rgb"),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    catalog::collect(&mut raw, &path, false).unwrap();
    fs::remove_file(work.path("rgb.icc")).unwrap();
    roles::evaluate(&raw, &path, Role::Working, &sample()).unwrap();
    let kit = work.path("kit");
    package::init(&kit, "role-kit").unwrap();
    raw["asset"] = json!({"schema":1,"id":"card","name":"Card"});
    fs::write(
        kit.join("assets/card.pen"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let output = work.path("role.penpkg");
    package::pack(&kit, &output).unwrap();
    let verified = package::verify(&output).unwrap();
    assert_eq!(
        verified["package"]["assets"]["card"]["icc_profiles"][0]["digest"],
        resource::sha256(RGB)
    );
}
