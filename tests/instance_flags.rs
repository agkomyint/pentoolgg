//! instance command flag coverage at the CLI level.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-instance-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(cwd: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn ok(cwd: &PathBuf, args: &[&str]) -> Value {
    let out = run(cwd, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn succeed(cwd: &PathBuf, args: &[&str]) {
    let out = run(cwd, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn instance_update_dry_run_echoes_dry_run_when_unchanged() {
    let ws = Workspace::new("dryrun-echo");
    let cwd = &ws.0;
    fs::create_dir_all(cwd.join("lib")).unwrap();
    succeed(
        cwd,
        &["new", "host.pen", "--width", "100", "--height", "100"],
    );
    ok(
        cwd,
        &[
            "shape", "host.pen", "rect", "r", "--layer", "layer-1", "--x", "2", "--y", "2",
            "--width", "20", "--height", "20", "--fill", "#ff0000",
        ],
    );
    ok(
        cwd,
        &[
            "asset",
            "create",
            "--id",
            "lib/r",
            "--name",
            "R",
            "host.pen",
            "lib/r.pen",
        ],
    );
    succeed(cwd, &["library", "add", "--name", "testlib", "lib"]);
    ok(cwd, &["library", "refresh"]);
    succeed(
        cwd,
        &["new", "dest.pen", "--width", "100", "--height", "100"],
    );
    ok(cwd, &["migrate", "dest.pen", "--target", "3"]);
    ok(cwd, &["add", "dest.pen", "lib/r", "--mode", "instance"]);
    let list = ok(cwd, &["instance", "dest.pen", "list"]);
    let id = list["instances"][0]["id"].as_str().unwrap().to_string();
    let report = ok(
        cwd,
        &[
            "instance",
            "dest.pen",
            "update",
            &id,
            "--source",
            "lib/r.pen",
            "--dry-run",
        ],
    );
    assert_eq!(report["changed"], false);
    assert_eq!(
        report["dry_run"], true,
        "dry-run rehearsal must echo dry_run:true, got: {report}"
    );
}
