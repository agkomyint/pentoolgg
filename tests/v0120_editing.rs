//! Paint assignment/conversion semantics, locks, shared users and atomic CLI edits.
use pentool::{
    color::{
        catalog,
        editing::{self, Operation, Options, Target},
        icc::Intent,
        value::{Destination, NativeColor},
    },
    composite, transaction,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
fn doc() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
fn target(space: &str) -> Target {
    Target::Native(Destination::parse(&json!({"space":space})).unwrap())
}
fn options(target: Target) -> Options<'static> {
    Options {
        operation: Operation::Convert,
        target,
        intent: Intent::RelativeColorimetric,
        page: None,
        all_pages: false,
        include_shared: false,
        offset: 0,
        limit: 100,
    }
}
fn paint(raw: &Value) -> &Value {
    &raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"]
}
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "pentool-color-edit-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("art.pen")
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn conversion_preserves_pixels_and_assignment_preserves_channel_numbers() {
    let raw = doc();
    let path = Path::new("target/paint-plan.pen");
    let before = composite::render(&raw, path, None, 1.0).unwrap();
    let converted = editing::plan(&raw, path, &options(target("lab-d50"))).unwrap();
    assert_eq!(converted.report["changed"], 2);
    assert_eq!(converted.report["clipped"], 0);
    assert_eq!(paint(&converted.document)["space"], "lab-d50");
    assert_eq!(
        composite::render(&converted.document, path, None, 1.0)
            .unwrap()
            .as_raw(),
        before.as_raw()
    );
    let mut assign = options(target("display-p3"));
    assign.operation = Operation::Assign;
    let assigned = editing::plan(&raw, path, &assign).unwrap();
    let original = NativeColor::from_legacy(paint(&raw)).unwrap().unwrap();
    assert_eq!(
        NativeColor::parse(paint(&assigned.document))
            .unwrap()
            .channels(),
        original.channels()
    );
    let converted = editing::plan(&raw, path, &options(target("display-p3"))).unwrap();
    assert_ne!(
        NativeColor::parse(paint(&converted.document))
            .unwrap()
            .channels(),
        original.channels()
    );
    assert_eq!(raw, doc());
}

#[test]
fn legacy_output_reports_quantization_and_allows_empty_catalog_downgrade() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"display-p3","channels":[1,0,0],"alpha":0.5});
    let result = editing::plan(&raw, Path::new("art.pen"), &options(Target::LegacySrgb)).unwrap();
    assert_eq!(paint(&result.document), "#ff000080");
    assert_eq!(result.report["clipped"], 1);
    assert_eq!(result.report["quantized"], 1);
    catalog::downgrade(result.document).unwrap();
    let mut assign = options(Target::LegacySrgb);
    assign.operation = Operation::Assign;
    assert!(editing::plan(&raw, Path::new("art.pen"), &assign).is_err());
}

#[test]
fn locked_nested_nodes_and_shared_alias_users_on_other_pages_are_respected() {
    let path = Path::new("art.pen");
    let mut raw = doc();
    let node = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    raw["pages"][0]["layers"][0]["nodes"] =
        json!([{"kind":"group","id":"locked-group","locked":true,"children":[node]}]);
    assert!(format!(
        "{:#}",
        editing::plan(&raw, path, &options(target("lab-d50"))).unwrap_err()
    )
    .contains("[locked]"));
    let mut raw = doc();
    raw["styles"] = json!({"base":{"type":"color","value":"#2468ac"},"alias":{"type":"color","value":{"ref":"base"}}});
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"] =
        json!({"ref":"alias","fallback":"none"});
    assert!(format!(
        "{:#}",
        editing::plan(&raw, path, &options(target("lab-d50"))).unwrap_err()
    )
    .contains("--include-shared"));
    let mut shared = options(target("lab-d50"));
    shared.include_shared = true;
    let planned = editing::plan(&raw, path, &shared).unwrap();
    assert_eq!(
        planned.document["styles"]["base"]["value"]["space"],
        "lab-d50"
    );
    let mut second = raw["pages"][0].clone();
    second["id"] = json!("second");
    second["layers"][0]["locked"] = json!(true);
    raw["pages"].as_array_mut().unwrap().push(second);
    let before = raw.clone();
    assert!(format!("{:#}", editing::plan(&raw, path, &shared).unwrap_err()).contains("[locked]"));
    assert_eq!(raw, before);
}

#[test]
fn selected_page_limits_pagination_and_missing_profiles_fail_or_report_exactly() {
    let mut raw = doc();
    let mut second = raw["pages"][0].clone();
    second["id"] = json!("second");
    second["layers"][0]["locked"] = json!(true);
    raw["pages"].as_array_mut().unwrap().push(second.clone());
    let mut opts = options(target("lab-d50"));
    opts.limit = 1;
    let result = editing::plan(&raw, Path::new("art.pen"), &opts).unwrap();
    assert_eq!(result.document["pages"][1], second);
    assert_eq!(result.report["matches"], 2);
    assert_eq!(result.report["returned"], 1);
    assert_eq!(result.report["has_more"], true);
    opts.offset = 2;
    let result = editing::plan(&raw, Path::new("art.pen"), &opts).unwrap();
    assert_eq!(result.report["returned"], 0);
    assert_eq!(result.report["changed"], 2);
    assert_eq!(result.report["has_more"], false);
    opts.limit = 0;
    assert!(editing::plan(&raw, Path::new("art.pen"), &opts).is_err());
    opts.limit = 1;
    opts.page = Some("missing");
    assert!(editing::plan(&raw, Path::new("art.pen"), &opts).is_err());
    opts.page = None;
    opts.all_pages = true;
    assert!(editing::plan(&raw, Path::new("art.pen"), &opts).is_err());
    let mut opts = options(Target::Native(
        Destination::parse(&json!({"space":"icc-cmyk","profile":"missing"})).unwrap(),
    ));
    opts.operation = Operation::Assign;
    assert!(format!(
        "{:#}",
        editing::plan(&doc(), Path::new("art.pen"), &opts).unwrap_err()
    )
    .contains("channel count"));
    opts.operation = Operation::Convert;
    assert!(editing::plan(&doc(), Path::new("art.pen"), &opts).is_err());
}

#[test]
fn assignment_can_replace_missing_external_interpretation_without_reading_old_bytes() {
    let work = Workspace::new();
    let mut raw = doc();
    let bytes = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
    std::fs::write(work.0.join("rgb.icc"), bytes).unwrap();
    catalog::import(
        &mut raw,
        &work.path(),
        bytes,
        "source",
        "allowed",
        None,
        Some("rgb.icc"),
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-rgb","profile":"source","channels":[0.2,0.4,0.6],"alpha":0.3});
    std::fs::remove_file(work.0.join("rgb.icc")).unwrap();
    assert!(editing::plan(&raw, &work.path(), &options(target("lab-d50"))).is_err());
    let mut opts = options(target("srgb"));
    opts.operation = Operation::Assign;
    let planned = editing::plan(&raw, &work.path(), &opts).unwrap();
    assert_eq!(paint(&planned.document)["channels"], json!([0.2, 0.4, 0.6]));
    assert_eq!(paint(&planned.document)["alpha"], 0.3);
}

#[test]
fn cli_dry_run_commit_revision_guard_and_undo_share_one_plan_and_history_entry() {
    let work = Workspace::new();
    let bytes = serde_json::to_vec(&doc()).unwrap();
    std::fs::write(work.path(), &bytes).unwrap();
    let args = [
        "color",
        "paints",
        "convert",
        "art.pen",
        "--to",
        r#"{"space":"lab-d50"}"#,
        "--limit",
        "1",
        "--json",
    ];
    let mut dry = args.to_vec();
    dry.push("--dry-run");
    let dry = work.ok(&dry);
    assert_eq!(std::fs::read(work.path()).unwrap(), bytes);
    let mut stale = args.to_vec();
    stale.extend(["--if-revision", "wrong"]);
    assert!(!work.run(&stale).status.success());
    assert_eq!(std::fs::read(work.path()).unwrap(), bytes);
    let committed = work.ok(&args);
    assert_eq!(committed["result"], dry["result"]);
    assert_eq!(
        committed["change"]["revision_after"],
        dry["change"]["revision_after"]
    );
    assert_eq!(
        pentool::history::list(&work.path()).unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    work.ok(&["undo", "art.pen", "--json"]);
    assert_eq!(std::fs::read(work.path()).unwrap(), bytes);
    work.ok(&[
        "color",
        "paints",
        "assign",
        "art.pen",
        "--to",
        r#"{"space":"display-p3"}"#,
        "--dry-run",
        "--json",
    ]);
    work.ok(&[
        "color",
        "paints",
        "convert",
        "art.pen",
        "--legacy-srgb",
        "--dry-run",
        "--json",
    ]);
    assert!(!work
        .run(&[
            "color",
            "paints",
            "convert",
            "art.pen",
            "--legacy-srgb",
            "--to",
            r#"{"space":"srgb"}"#
        ])
        .status
        .success());
}

#[test]
fn stale_plans_never_partially_commit() {
    let work = Workspace::new();
    let raw = doc();
    let bytes = serde_json::to_vec(&raw).unwrap();
    std::fs::write(work.path(), &bytes).unwrap();
    let plan = editing::plan(&raw, &work.path(), &options(target("lab-d50"))).unwrap();
    let mut changed = raw.clone();
    changed["user_edit"] = json!("keep");
    let newer = serde_json::to_vec(&changed).unwrap();
    std::fs::write(work.path(), &newer).unwrap();
    assert!(transaction::commit_value(
        &work.path(),
        "color-paints-convert",
        false,
        Some(&transaction::revision(&bytes)),
        &plan.document
    )
    .is_err());
    assert_eq!(std::fs::read(work.path()).unwrap(), newer);
}

#[test]
fn immutable_vector_mask_conversion_replaces_hashes_aliases_and_all_users_atomically() {
    let mut raw = doc();
    let path = Path::new("target/mask-paints.pen");
    pentool::mask::create(&mut raw, path, None, "clip", "vector:color-card").unwrap();
    pentool::mask::attach(&mut raw, None, "color-card", "clip", &json!({})).unwrap();
    let old = raw["masks"]["clip"].as_str().unwrap().to_owned();
    let before = composite::render(&raw, path, None, 1.0).unwrap();
    let mut opts = options(target("lab-d50"));
    assert!(
        format!("{:#}", editing::plan(&raw, path, &opts).unwrap_err()).contains("--include-shared")
    );
    opts.include_shared = true;
    let plan = editing::plan(&raw, path, &opts).unwrap();
    let new = plan.document["masks"]["clip"].as_str().unwrap();
    assert_ne!(old, new);
    assert!(plan.document["mask_resources"].get(&old).is_none());
    assert_eq!(
        plan.document["pages"][0]["layers"][0]["nodes"][0]["mask"]["resource"],
        new
    );
    assert_eq!(
        plan.report["resource_replacements"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    pentool::scene::validate(&plan.document).unwrap();
    assert_eq!(
        before.as_raw(),
        composite::render(&plan.document, path, None, 1.0)
            .unwrap()
            .as_raw()
    );
    opts.target = Target::LegacySrgb;
    opts.all_pages = true;
    let legacy = editing::plan(&plan.document, path, &opts).unwrap();
    catalog::downgrade(legacy.document).unwrap();
    let mut other = raw["pages"][0].clone();
    other["id"] = json!("other");
    other["layers"][0]["locked"] = json!(true);
    raw["pages"].as_array_mut().unwrap().push(other);
    opts.all_pages = false;
    assert!(
        format!("{:#}", editing::plan(&raw, path, &opts).unwrap_err())
            .contains("mask used by locked content")
    );
    assert_eq!(raw["masks"]["clip"], old);
}

#[test]
fn native_mask_profiles_and_shared_styles_validate_without_loading_unrelated_pages() {
    let mut raw = doc();
    let path = Path::new("target/mask-native.pen");
    pentool::mask::create(&mut raw, path, None, "clip", "vector:color-card").unwrap();
    pentool::mask::attach(&mut raw, None, "color-card", "clip", &json!({})).unwrap();
    let old = raw["masks"]["clip"].as_str().unwrap().to_owned();
    let before = composite::render(&raw, path, None, 1.0).unwrap();
    let mut resource = raw["mask_resources"][&old].clone();
    resource["node"]["style"]["fill"] = json!({"ref":"mask-color","fallback":"#000000"});
    let hash = pentool::asset::hash_bytes(&serde_json::to_vec(&resource).unwrap());
    raw["mask_resources"].as_object_mut().unwrap().remove(&old);
    raw["mask_resources"][&hash] = resource;
    raw["masks"]["clip"] = json!(hash);
    raw["pages"][0]["layers"][0]["nodes"][0]["mask"]["resource"] = json!(hash);
    raw["styles"] = json!({"mask-color":{"type":"color","value":{"space":"lab-d50","channels":[30,10,10],"alpha":1}}});
    pentool::scene::validate(&raw).unwrap();
    assert_eq!(
        before.as_raw(),
        composite::render(&raw, path, None, 1.0).unwrap().as_raw()
    );
    raw["styles"]["mask-color"]["value"] =
        json!({"space":"icc-rgb","profile":"absent","channels":[0,0,0]});
    assert!(pentool::scene::validate(&raw).is_err());
}

#[test]
fn missing_style_fallbacks_convert_but_unsupported_used_styles_are_not_silently_ignored() {
    let mut raw = doc();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"] =
        json!({"ref":"absent","fallback":"#2468ac"});
    let result = editing::plan(&raw, Path::new("art.pen"), &options(target("lab-d50"))).unwrap();
    assert_eq!(paint(&result.document)["space"], "lab-d50");
    raw["styles"] = json!({"absent":{"type":"color","value":{"ref":"unknown"}},"unknown":{"type":"extension-kind","value":"#2468ac"}});
    let mut opts = options(target("lab-d50"));
    opts.all_pages = true;
    opts.include_shared = true;
    assert!(format!(
        "{:#}",
        editing::plan(&raw, Path::new("art.pen"), &opts).unwrap_err()
    )
    .contains("supported color-bearing style"));
}
