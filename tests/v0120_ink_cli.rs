use pentool::color::{
    catalog,
    icc::Intent,
    roles::{self, Role},
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pentool-ink-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn document(&self) -> PathBuf {
        self.0.join("art.pen")
    }
    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn inspect_ink_profile_override_is_read_only_and_paginated() {
    let space = Workspace::new();
    let path = space.document();
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    let imported = catalog::import(
        &mut raw,
        &path,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let profile = imported["profile"].as_str().unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["fallback"] =
        json!({"space":"icc-cmyk","profile":profile,"channels":[0.4,0.4,0.3,0.9]});
    let bytes = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &bytes).unwrap();
    let output = space.run(&[
        "inspect",
        "ink",
        "art.pen",
        "--profile",
        "profile:press",
        "--tac-limit",
        "300",
        "--small-text-points",
        "8",
        "--units-per-inch",
        "72",
        "--limit",
        "1",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["scope"], "visible-paint-samples");
    assert_eq!(report["summary"]["sample_count"], 2);
    assert_eq!(report["returned"], 1);
    assert_eq!(report["has_more"], true);
    assert_eq!(report["output_profile"], profile);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let tail = space.run(&[
        "inspect",
        "ink",
        "art.pen",
        "--profile",
        "press",
        "--tac-limit",
        "300",
        "--small-text-points",
        "8",
        "--units-per-inch",
        "72",
        "--offset",
        "1",
        "--limit",
        "1",
    ]);
    assert!(
        tail.status.success(),
        "{}",
        String::from_utf8_lossy(&tail.stderr)
    );
    let report: Value = serde_json::from_slice(&tail.stdout).unwrap();
    assert_eq!(report["samples"][0]["channels"]["c"], 0.4);
    assert_eq!(report["samples"][0]["rich_black"], true);
    assert_eq!(report["has_more"], false);

    roles::assign(
        &mut raw,
        &path,
        Role::Output,
        &json!({"space":"icc-cmyk","profile":"press"}),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    let assigned = serde_json::to_vec(&raw).unwrap();
    fs::write(&path, &assigned).unwrap();
    let configured = space.run(&[
        "inspect",
        "ink",
        "art.pen",
        "--tac-limit",
        "300",
        "--small-text-points",
        "8",
        "--units-per-inch",
        "72",
        "--limit",
        "2",
    ]);
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let report: Value = serde_json::from_slice(&configured.stdout).unwrap();
    assert_eq!(report["output_profile"], profile);
    assert_eq!(report["returned"], 2);
    assert_eq!(fs::read(&path).unwrap(), assigned);
}

#[test]
fn inspect_ink_rejects_missing_output_and_bad_limits() {
    let space = Workspace::new();
    fs::write(
        space.document(),
        include_bytes!("../docs/fixtures/print-v8.pen"),
    )
    .unwrap();
    let base = [
        "inspect",
        "ink",
        "art.pen",
        "--tac-limit",
        "300",
        "--small-text-points",
        "8",
        "--units-per-inch",
        "72",
    ];
    let absent = space.run(&base);
    assert!(!absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stderr).contains("CMYK output role"));
    let bad = space.run(&[
        "inspect",
        "ink",
        "art.pen",
        "--tac-limit",
        "401",
        "--small-text-points",
        "8",
        "--units-per-inch",
        "72",
    ]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("TAC limit"));
}
