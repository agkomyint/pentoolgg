//! v0.28.0: `pentool report` builds cited, charted documents from a spec and data files.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pentool-report-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(args: &[&str]) -> Value {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn fail(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} should fail");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn spec_with(dir: &std::path::Path, spec: &str) -> String {
    fs::write(
        dir.join("regions.csv"),
        "region,revenue\nAsia,\"$84.1\"\nEurope,31.0\nOther,9.9\n",
    )
    .unwrap();
    let path = dir.join("spec.json");
    fs::write(&path, spec).unwrap();
    path.to_string_lossy().into_owned()
}

const GOOD: &str = r##"{"title":"T","sources":[
 {"id":"a","title":"A","publisher":"Pub","kind":"observed","data":"regions.csv"},
 {"id":"f","title":"F","kind":"forecast"}],
 "pages":[{"title":"One","blocks":[
 {"type":"stats","items":[{"value":"1","label":"x","source":"a"}]},
 {"type":"bar","title":"Rev","source":"a","label":"region","value":"revenue","prefix":"$","sort":"desc"},
 {"type":"column","source":"a"},{"type":"segmented","source":"a"},{"type":"table","source":"a"},
 {"type":"callout","text":"c","cite":["f"]}]}]}"##;

#[test]
fn builds_a_valid_document_with_numbered_citations_and_a_sources_page() {
    let dir = temp("build");
    let spec = spec_with(&dir, GOOD);
    let out = dir.join("r.pen").to_string_lossy().into_owned();
    let report = ok(&["report", "build", &spec, "--out", &out, "--strict"]);
    assert_eq!(report["citations"][0]["id"], "a");
    assert_eq!(report["citations"][1]["id"], "f");
    assert_eq!(report["warnings"].as_array().unwrap().len(), 0, "{report}");
    let doc: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    let pages = doc["pages"].as_array().unwrap();
    assert_eq!(pages.last().unwrap()["name"], "Sources");
    assert!(
        ok(&["tree", &out, "--limit", "5"])["matches"]
            .as_u64()
            .unwrap()
            > 20
    );
    let text = serde_json::to_string(&doc).unwrap();
    assert!(
        text.contains("$84.1") && text.contains("[1]"),
        "values and marks"
    );
    // Sorted descending: Asia's bar is the first bar label.
    assert!(text.find("Asia").unwrap() < text.find("Europe").unwrap());
}

#[test]
fn unknown_source_missing_column_and_path_escape_fail_without_writing() {
    let dir = temp("errors");
    let out = dir.join("r.pen").to_string_lossy().into_owned();
    let bad_source = GOOD.replace("\"cite\":[\"f\"]", "\"cite\":[\"nope\"]");
    let spec = spec_with(&dir, &bad_source);
    assert!(fail(&["report", "build", &spec, "--out", &out]).contains("unknown source"));
    let bad_col = GOOD.replace("\"value\":\"revenue\"", "\"value\":\"missing\"");
    let spec = spec_with(&dir, &bad_col);
    assert!(fail(&["report", "build", &spec, "--out", &out]).contains("no column"));
    let escape = GOOD.replace("regions.csv\"}", "../regions.csv\"}");
    let spec = spec_with(&dir, &escape);
    assert!(fail(&["report", "build", &spec, "--out", &out]).contains("may not use"));
    assert!(!dir.join("r.pen").exists());
}

#[test]
fn strict_mode_flags_uncited_numbers_and_unused_sources() {
    let dir = temp("strict");
    let loose = r##"{"title":"T","sources":[{"id":"u","title":"Unused"}],
     "pages":[{"blocks":[{"type":"stats","items":[{"value":"9","label":"no source"}]}]}]}"##;
    let spec = spec_with(&dir, loose);
    let out = dir.join("r.pen").to_string_lossy().into_owned();
    let report = ok(&["report", "check", &spec]);
    assert_eq!(report["warnings"].as_array().unwrap().len(), 2, "{report}");
    assert!(fail(&["report", "check", &spec, "--strict"]).contains("strict"));
    assert!(fail(&["report", "build", &spec, "--out", &out, "--strict"])
        .contains("nothing was written"));
    assert!(!dir.join("r.pen").exists());
}

#[test]
fn long_content_flows_onto_continuation_pages() {
    let dir = temp("flow");
    let blocks: Vec<String> = (0..40)
        .map(|i| format!(r#"{{"type":"text","text":"paragraph {i} lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua"}}"#))
        .collect();
    let spec = spec_with(
        &dir,
        &format!(
            r#"{{"title":"T","pages":[{{"title":"Long","blocks":[{}]}}]}}"#,
            blocks.join(",")
        ),
    );
    let out = dir.join("r.pen").to_string_lossy().into_owned();
    let report = ok(&["report", "build", &spec, "--out", &out]);
    assert!(report["pages"].as_u64().unwrap() > 1, "{report}");
    let doc: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert!(doc["pages"][1]["name"]
        .as_str()
        .unwrap()
        .contains("(cont.)"));
}

#[test]
fn data_command_describes_columns_and_example_is_runnable() {
    let dir = temp("data");
    spec_with(&dir, GOOD);
    let csv = dir.join("regions.csv").to_string_lossy().into_owned();
    let summary = ok(&["report", "data", &csv, "--rows", "2"]);
    assert_eq!(summary["rows"], 3);
    assert_eq!(summary["columns"][1]["numeric"], true);
    assert_eq!(summary["columns"][0]["numeric"], false);
    assert_eq!(summary["sample"].as_array().unwrap().len(), 2);
    let example = ok(&["report", "example"]);
    fs::write(dir.join("ex.json"), example["spec"].to_string()).unwrap();
    fs::write(
        dir.join("regions.csv"),
        example["regions.csv"].as_str().unwrap(),
    )
    .unwrap();
    let spec = dir.join("ex.json").to_string_lossy().into_owned();
    ok(&["report", "check", &spec, "--strict"]);
}

#[test]
fn existing_output_needs_overwrite_and_dry_run_writes_nothing() {
    let dir = temp("overwrite");
    let spec = spec_with(&dir, GOOD);
    let out = dir.join("r.pen").to_string_lossy().into_owned();
    ok(&["report", "build", &spec, "--out", &out, "--dry-run"]);
    assert!(!dir.join("r.pen").exists());
    ok(&["report", "build", &spec, "--out", &out]);
    assert!(fail(&["report", "build", &spec, "--out", &out]).contains("--overwrite"));
    ok(&["report", "build", &spec, "--out", &out, "--overwrite"]);
}
