//! v0.19.0 design systems: token resolution limits and library packages,
//! locks, update planning, and diff reports (`docs/design-systems-v1.md`).
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU32, Ordering},
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-design-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn s(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(ws: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&ws.0)
        .args(args)
        .output()
        .unwrap()
}

fn ok(ws: &Workspace, args: &[&str]) -> Value {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{args:?} printed non-JSON ({e}): {text}"))
}

/// Run a command that must succeed and ignore its output.
fn quiet(ws: &Workspace, args: &[&str]) {
    let out = run(ws, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A failing design command: the structured `{"error": {...}}` object.
fn err(ws: &Workspace, args: &[&str]) -> Value {
    let out = run(ws, args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    serde_json::from_str::<Value>(&text)
        .unwrap_or_else(|e| panic!("{args:?} stderr is not JSON ({e}): {text}"))["error"]
        .clone()
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Build `brand-<version>.penpkg` from the given library tokens.
fn library(ws: &Workspace, version: &str, tokens: Value) -> PathBuf {
    let dir = ws.path(&format!("lib-{version}"));
    if !dir.exists() {
        quiet(
            ws,
            &["design", "library", "init", &text(&dir), "acme/brand"],
        );
    }
    let manifest_path = dir.join("pentool.package.json");
    let mut manifest = read(&manifest_path);
    manifest["version"] = json!(version);
    write(&manifest_path, &manifest);
    let asset = dir.join("assets/acme-brand.pen");
    let mut doc = read(&asset);
    doc["design_library"]["version"] = json!(version);
    doc["design_library"]["tokens"] = tokens;
    write(&asset, &doc);
    let out = ws.path(&format!("brand-{version}.penpkg"));
    quiet(ws, &["design", "library", "pack", &text(&dir), &text(&out)]);
    out
}

fn color(value: &str) -> Value {
    json!({"type":"color","value":value})
}

fn base_tokens() -> Value {
    let mut tokens = serde_json::Map::new();
    for index in 0..20 {
        tokens.insert(
            format!("color.c{index:02}"),
            color(&format!("#0000{index:02x}")),
        );
    }
    tokens.insert("color.accent".into(), color("#f59e0b"));
    tokens.insert(
        "color.cta".into(),
        json!({"type":"color","value":{"ref":"color.accent"}}),
    );
    tokens.insert("space.md".into(), json!({"type":"spacing","value":16}));
    Value::Object(tokens)
}

fn rect(id: &str, token: &str) -> Value {
    json!({"kind":"rect","id":id,"x":0,"y":0,"width":10,"height":10,"radius_x":0,"radius_y":0,
           "style":{"fill":{"ref":token,"fallback":"#000000"}}})
}

fn document(ws: &Workspace) -> PathBuf {
    let path = ws.path("doc.pen");
    quiet(ws, &["new", &text(&path)]);
    let mut doc = read(&path);
    doc["pages"][0]["layers"][0]["nodes"] = json!([
        rect("hero", "color.accent"),
        rect("button", "color.cta"),
        rect("other", "color.c01"),
        rect("plain", "color.c02"),
    ]);
    write(&path, &doc);
    path
}

fn install(ws: &Workspace, package: &Path, doc: &Path) -> Value {
    ok(
        ws,
        &["design", "library", "install", &text(package), &text(doc)],
    )
}

#[test]
fn library_pack_is_deterministic_and_verified() {
    let ws = Workspace::new("pack");
    let first = library(&ws, "1.0.0", base_tokens());
    let bytes = fs::read(&first).unwrap();
    let second = ws.path("second.penpkg");
    quiet(
        &ws,
        &[
            "design",
            "library",
            "pack",
            &ws.s("lib-1.0.0"),
            &text(&second),
        ],
    );
    assert_eq!(
        bytes,
        fs::read(&second).unwrap(),
        "packs must be byte-identical"
    );
    let report = ok(&ws, &["design", "library", "inspect", &text(&first)]);
    assert_eq!(report["name"], "acme/brand");
    assert!(report["package_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    quiet(&ws, &["design", "library", "verify", &text(&first)]);
}

#[test]
fn tampered_package_is_rejected_offline_and_leaves_document_unchanged() {
    let ws = Workspace::new("tamper");
    let package = library(&ws, "1.0.0", base_tokens());
    let mut bytes = fs::read(&package).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    let bad = ws.path("bad.penpkg");
    fs::write(&bad, &bytes).unwrap();
    let doc = document(&ws);
    let before = fs::read(&doc).unwrap();
    let error = err(
        &ws,
        &["design", "library", "install", &text(&bad), &text(&doc)],
    );
    assert!(
        ["hash-mismatch", "invalid-input"].contains(&error["code"].as_str().unwrap()),
        "{error}"
    );
    assert_eq!(before, fs::read(&doc).unwrap());
    assert!(!ws.path("pentool.lock").exists());
}

#[test]
fn lock_verify_and_status_cover_installed_libraries() {
    let ws = Workspace::new("lock");
    let package = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &package, &doc);
    let lock = ok(&ws, &["lock", "verify"]);
    assert_eq!(lock["ok"], true, "{lock}");
    let status = ok(&ws, &["design", "library", "status", &text(&doc)]);
    assert_eq!(status["libraries"][0]["state"], "ok", "{status}");
}

#[test]
fn tokens_update_changes_exactly_dependents() {
    let ws = Workspace::new("exact");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &v1, &doc);
    let before = read(&doc);

    let mut tokens = base_tokens();
    tokens["color.accent"] = color("#d97706");
    let v2 = library(&ws, "1.1.0", tokens);
    let d = text(&doc);
    let p = text(&v2);
    let plan = ok(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--dry-run",
        ],
    );
    assert_eq!(plan["writes"], json!(["color.accent"]));
    assert_eq!(plan["counts"]["changed"], 1);
    assert_eq!(plan["counts"]["unchanged"], 22);
    let row = plan["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["token"] == "color.accent")
        .unwrap();
    assert_eq!(row["dependents"]["count"], 3);
    assert_eq!(row["dependents"]["tokens"], json!(["color.cta"]));
    assert_eq!(read(&doc), before, "dry run must not write");

    quiet(&ws, &["design", "library", "update", &d, "--package", &p]);
    let after = read(&doc);
    // Only the accent record and the library record differ; every node is untouched.
    assert_eq!(
        after["pages"], before["pages"],
        "nodes are untouched by a token update"
    );
    let mut expected = before.clone();
    expected["styles"]["color.accent"] = after["styles"]["color.accent"].clone();
    expected["design"] = after["design"].clone();
    assert_eq!(after["styles"], expected["styles"]);
    assert_eq!(after["styles"]["color.accent"]["value"], "#d97706");
    assert_eq!(
        after["design"]["libraries"]["acme/brand"]["version"],
        "1.1.0"
    );
    assert_eq!(
        after["styles"]["color.accent"]["source"]["version"],
        "1.1.0"
    );

    let cta = ok(&ws, &["design", "token", "resolve", &d, "color.cta"]);
    assert_eq!(cta["value"], "#d97706");
    assert_eq!(cta["chain"], json!(["color.cta", "color.accent"]));
    let svg = ws.path("out.svg");
    quiet(&ws, &["export", &d, &text(&svg)]);
    let svg = fs::read_to_string(svg).unwrap().to_lowercase();
    assert!(svg.contains("#d97706"), "{svg}");
}

#[test]
fn local_override_is_kept_and_conflicts_block_the_commit() {
    let ws = Workspace::new("override");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &v1, &doc);
    let d = text(&doc);
    quiet(
        &ws,
        &[
            "style",
            &d,
            "set",
            "color.c01",
            "--type",
            "color",
            "--value",
            "#111111",
        ],
    );
    quiet(
        &ws,
        &[
            "style",
            &d,
            "set",
            "color.c02",
            "--type",
            "color",
            "--value",
            "#222222",
        ],
    );

    let mut tokens = base_tokens();
    tokens["color.accent"] = color("#d97706");
    tokens["color.c02"] = color("#333333");
    let v2 = library(&ws, "2.0.0", tokens);
    let p = text(&v2);

    let plan = ok(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--dry-run",
        ],
    );
    assert_eq!(plan["counts"]["kept-override"], 1, "{plan}");
    assert_eq!(plan["counts"]["conflict"], 1);
    assert_eq!(plan["resolutions_needed"], json!(["color.c02"]));
    assert_eq!(plan["would_change"], false);

    let before = fs::read(&doc).unwrap();
    let error = err(&ws, &["design", "library", "update", &d, "--package", &p]);
    assert_eq!(error["code"], "conflict");
    assert_eq!(
        before,
        fs::read(&doc).unwrap(),
        "blocked update must not write"
    );

    quiet(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--continue-on-conflict",
        ],
    );
    let after = read(&doc);
    assert_eq!(after["styles"]["color.accent"]["value"], "#d97706");
    assert_eq!(after["styles"]["color.c01"]["value"], "#111111");
    assert_eq!(after["styles"]["color.c02"]["value"], "#222222");

    let resolutions = ws.path("res.json");
    write(
        &resolutions,
        &json!([{"token":"color.c02","action":"take-source"}]),
    );
    quiet(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--resolutions",
            &text(&resolutions),
        ],
    );
    let after = read(&doc);
    assert_eq!(after["styles"]["color.c02"]["value"], "#333333");
    assert_eq!(
        after["styles"]["color.c01"]["value"], "#111111",
        "override survives"
    );

    write(
        &resolutions,
        &json!([{"token":"color.nope","action":"keep-local"}]),
    );
    let error = err(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--resolutions",
            &text(&resolutions),
            "--dry-run",
        ],
    );
    assert_eq!(error["code"], "unknown-token");
}

#[test]
fn dry_run_equals_commit_plan() {
    let ws = Workspace::new("dry");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &v1, &doc);
    let mut tokens = base_tokens();
    tokens["color.accent"] = color("#d97706");
    tokens["color.new"] = color("#abcdef");
    let v2 = library(&ws, "1.1.0", tokens);
    let d = text(&doc);
    let p = text(&v2);
    let dry = ok(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--dry-run",
        ],
    );
    let real = ok(&ws, &["design", "library", "update", &d, "--package", &p]);
    let strip = |mut v: Value| {
        v.as_object_mut().unwrap().remove("dry_run");
        v.as_object_mut().unwrap().remove("change");
        v
    };
    assert_eq!(dry["dry_run"], true);
    assert_eq!(real["dry_run"], false);
    assert_eq!(strip(dry), strip(real.clone()));
    assert_eq!(real["writes"], json!(["color.accent", "color.new"]));
    let again = ok(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--dry-run",
        ],
    );
    assert_eq!(again["would_change"], false);
}

#[test]
fn removed_tokens_need_a_resolution_and_map_target_rewrites_refs() {
    let ws = Workspace::new("removed");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &v1, &doc);
    let mut tokens = base_tokens();
    tokens.as_object_mut().unwrap().remove("color.c01");
    let v2 = library(&ws, "2.0.0", tokens);
    let d = text(&doc);
    let p = text(&v2);
    let plan = ok(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--dry-run",
        ],
    );
    assert_eq!(plan["resolutions_needed"], json!(["color.c01"]));
    let resolutions = ws.path("res.json");
    write(
        &resolutions,
        &json!([{"token":"color.c01","action":"map-target","to":"color.accent"}]),
    );
    quiet(
        &ws,
        &[
            "design",
            "library",
            "update",
            &d,
            "--package",
            &p,
            "--resolutions",
            &text(&resolutions),
        ],
    );
    let after = read(&doc);
    assert!(after["styles"].get("color.c01").is_none());
    assert_eq!(
        after["pages"][0]["layers"][0]["nodes"][2]["style"]["fill"]["ref"],
        "color.accent"
    );
}

#[test]
fn pagination_fields_are_exact() {
    let ws = Workspace::new("page");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let doc = document(&ws);
    install(&ws, &v1, &doc);
    let d = text(&doc);
    let first = ok(&ws, &["design", "token", "list", &d, "--limit", "10"]);
    assert_eq!(first["matches"], 23);
    assert_eq!(first["returned"], 10);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["limit"], 10);
    assert_eq!(first["has_more"], true);
    let last = ok(
        &ws,
        &[
            "design", "token", "list", &d, "--limit", "10", "--offset", "20",
        ],
    );
    assert_eq!(last["returned"], 3);
    assert_eq!(last["has_more"], false);
    let mut ids: Vec<String> = Vec::new();
    for offset in ["0", "10", "20"] {
        let page = ok(
            &ws,
            &[
                "design", "token", "list", &d, "--limit", "10", "--offset", offset,
            ],
        );
        ids.extend(
            page["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["token"].as_str().unwrap().to_owned()),
        );
    }
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "tokens are sorted bytewise");
    assert_eq!(ids.len(), 23);
    assert_eq!(
        err(&ws, &["design", "token", "list", &d, "--limit", "0"])["code"],
        "invalid-input"
    );
    assert_eq!(
        err(&ws, &["design", "token", "list", &d, "--limit", "1001"])["code"],
        "invalid-input"
    );

    let mut tokens = base_tokens();
    tokens["color.accent"] = color("#000001");
    for index in 0..5 {
        tokens[format!("color.d{index}")] = color("#010101");
    }
    let v2 = library(&ws, "1.1.0", tokens);
    let diff = ok(
        &ws,
        &[
            "design",
            "library",
            "diff",
            &text(&v1),
            &text(&v2),
            "--limit",
            "4",
            "--offset",
            "3",
        ],
    );
    assert_eq!(diff["matches"], 6);
    assert_eq!(diff["returned"], 3);
    assert_eq!(diff["has_more"], false);
    assert_eq!(diff["semver"]["required"], "minor");
    assert_eq!(diff["semver"]["ok"], true);
}

#[test]
fn pack_enforces_semver_against_previous_release() {
    let ws = Workspace::new("semver");
    let v1 = library(&ws, "1.0.0", base_tokens());
    let dir = ws.path("lib-1.0.1");
    drop(library(&ws, "1.0.1", base_tokens()));
    let asset = dir.join("assets/acme-brand.pen");
    let mut doc = read(&asset);
    doc["design_library"]["tokens"]
        .as_object_mut()
        .unwrap()
        .remove("space.md");
    write(&asset, &doc);
    let error = err(
        &ws,
        &[
            "design",
            "library",
            "pack",
            &text(&dir),
            &ws.s("x.penpkg"),
            "--previous",
            &text(&v1),
        ],
    );
    assert_eq!(error["code"], "invalid-input");
    assert!(
        error["message"].as_str().unwrap().contains("major"),
        "{error}"
    );
}

#[test]
fn alias_cycle_depth_and_count_limits_use_the_error_shape() {
    let ws = Workspace::new("limits");
    let dir = ws.path("cyc");
    quiet(
        &ws,
        &["design", "library", "init", &text(&dir), "acme/brand"],
    );
    let asset = dir.join("assets/acme-brand.pen");
    let mut doc = read(&asset);
    doc["design_library"]["tokens"] = json!({
        "color.a": {"type":"color","value":{"ref":"color.b"}},
        "color.b": {"type":"color","value":{"ref":"color.a"}},
    });
    write(&asset, &doc);
    let error = err(
        &ws,
        &["design", "library", "pack", &text(&dir), &ws.s("c.penpkg")],
    );
    assert_eq!(error["code"], "token-cycle");
    assert!(error["message"].as_str().unwrap().contains("cycle"));
    assert!(error["path"].is_array(), "{error}");
    assert!(error["action"].is_string());
    assert_eq!(error["operation"], "design library pack");

    let mut tokens = serde_json::Map::new();
    for index in 0..10 {
        tokens.insert(
            format!("space.s{index}"),
            json!({"type":"spacing","value":{"ref":format!("space.s{}", index + 1)}}),
        );
    }
    tokens.insert("space.s10".into(), json!({"type":"spacing","value":4}));
    doc["design_library"]["tokens"] = Value::Object(tokens);
    write(&asset, &doc);
    let error = err(
        &ws,
        &["design", "library", "pack", &text(&dir), &ws.s("d.penpkg")],
    );
    assert_eq!(error["code"], "limit-exceeded");

    let mut tokens = serde_json::Map::new();
    for index in 0..5001 {
        tokens.insert(
            format!("space.t{index}"),
            json!({"type":"spacing","value":1}),
        );
    }
    doc["design_library"]["tokens"] = Value::Object(tokens);
    write(&asset, &doc);
    let error = err(
        &ws,
        &["design", "library", "pack", &text(&dir), &ws.s("e.penpkg")],
    );
    assert_eq!(error["code"], "limit-exceeded");

    // `style set` still rejects cycles with the legacy message and writes nothing.
    let target = document(&ws);
    let d = text(&target);
    quiet(
        &ws,
        &[
            "style", &d, "set", "a", "--type", "color", "--value", "#112233",
        ],
    );
    quiet(
        &ws,
        &[
            "style",
            &d,
            "set",
            "b",
            "--type",
            "color",
            "--value",
            "{\"ref\":\"a\"}",
        ],
    );
    let before = fs::read(&target).unwrap();
    let out = run(
        &ws,
        &[
            "style",
            &d,
            "set",
            "a",
            "--type",
            "color",
            "--value",
            "{\"ref\":\"b\"}",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("style alias cycle: a -> b -> a"));
    assert_eq!(before, fs::read(&target).unwrap());
}

#[test]
fn resolution_is_offline_deterministic_and_falls_back() {
    let ws = Workspace::new("resolve");
    let doc = document(&ws);
    let mut raw = read(&doc);
    let mut styles = serde_json::Map::new();
    for index in 0..10 {
        styles.insert(
            format!("space.s{index}"),
            json!({"type":"spacing","value":{"ref":format!("space.s{}", index + 1)}}),
        );
    }
    styles.insert("space.s10".into(), json!({"type":"spacing","value":4}));
    styles.insert("color.ok".into(), color("#336699"));
    styles.insert(
        "color.alias".into(),
        json!({"type":"color","value":{"ref":"color.ok"}}),
    );
    styles.insert(
        "color.dangling".into(),
        json!({"type":"color","value":{"ref":"color.gone"}}),
    );
    raw["styles"] = Value::Object(styles);
    raw["pages"][0]["layers"][0]["nodes"] = json!([rect("a", "color.alias")]);
    write(&doc, &raw);
    let d = text(&doc);
    assert_eq!(
        ok(&ws, &["design", "token", "resolve", &d, "space.s0"])["status"],
        "invalid"
    );
    let near = ok(&ws, &["design", "token", "resolve", &d, "space.s2"]);
    assert_eq!(near["status"], "resolved");
    assert_eq!(near["value"], 4);
    assert_eq!(
        ok(&ws, &["design", "token", "resolve", &d, "color.dangling"])["status"],
        "unresolved"
    );
    assert_eq!(
        err(&ws, &["design", "token", "resolve", &d, "color.zzz"])["code"],
        "unknown-token"
    );
    assert_eq!(
        near,
        ok(&ws, &["design", "token", "resolve", &d, "space.s2"])
    );
    let svg = ws.path("o.svg");
    quiet(&ws, &["export", &d, &text(&svg)]);
    let first = fs::read_to_string(&svg).unwrap().to_lowercase();
    assert!(first.contains("#336699"), "{first}");
    quiet(&ws, &["export", &d, &text(&svg)]);
    assert_eq!(first, fs::read_to_string(&svg).unwrap().to_lowercase());
}
