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

fn brand_tokens() -> Value {
    json!({
        "color.accent": color("#f59e0b"),
        "color.dark": color("#111111"),
        "color.unused": color("#123456"),
        "type.heading": {"type":"typography","value":{"font":"Inter","size":32,"weight":700}},
    })
}

fn brand_block() -> Value {
    json!({
        "colors": {"primary": {"token":"color.accent","usage":"CTAs","forbidden_on":["color.dark"]}},
        "type": {"heading": {"token":"type.heading"}},
        "logos": [{"id":"logo/primary","asset":"logo-primary","variants":["full","mark"],
                   "min_width":24,"clear_space":0.5,"forbidden_backgrounds":["color.accent"]}],
        "inks": {"spot": [{"id":"ink/pantone-1655","token":"color.accent"}],
                 "profiles": [{"role":"output","profile":format!("sha256:{}", "ab".repeat(32)),
                               "redistribution":"allowed"}]},
        "rules": [{"id":"min-contrast","kind":"contrast","min":4.5},
                  {"id":"fonts","kind":"fonts","allowed":["type.heading"]}],
    })
}

/// Pack `brand-<version>.penpkg` carrying `brand`; returns the pack command output.
fn brand_library(ws: &Workspace, version: &str, brand: Value, extra: &[&str]) -> (PathBuf, Output) {
    let dir = ws.path(&format!("blib-{version}"));
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
    doc["design_library"]["tokens"] = brand_tokens();
    doc["design_library"]["brand"] = brand;
    write(&asset, &doc);
    let out = ws.path(&format!("brand-kit-{version}.penpkg"));
    let mut args = vec!["design", "library", "pack"];
    let (d, o) = (text(&dir), text(&out));
    args.extend([d.as_str(), o.as_str()]);
    args.extend(extra);
    (out.clone(), run(ws, &args))
}

#[test]
fn brand_apply_vendors_only_referenced_tokens_and_show_resolves() {
    let ws = Workspace::new("brand-apply");
    let (pkg, out) = brand_library(&ws, "1.0.0", brand_block(), &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let packed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(packed["brand"], true);
    let doc = document(&ws);
    let d = text(&doc);
    assert_eq!(ok(&ws, &["design", "brand", "show", &d])["present"], false);
    let before = fs::read(&doc).unwrap();
    let dry = ok(
        &ws,
        &["design", "brand", "apply", &text(&pkg), &d, "--dry-run"],
    );
    assert_eq!(dry["would_change"], true);
    assert_eq!(dry["brand"]["profiles"][0]["state"], "missing");
    assert_eq!(before, fs::read(&doc).unwrap());
    ok(&ws, &["design", "brand", "apply", &text(&pkg), &d]);
    let raw = read(&doc);
    assert!(raw["styles"].get("color.accent").is_some());
    assert!(raw["styles"].get("type.heading").is_some());
    // Tokens the brand kit does not name are not vendored.
    assert!(raw["styles"].get("color.unused").is_none());
    assert_eq!(raw["design"]["brand"], brand_block());
    let shown = ok(&ws, &["design", "brand", "show", &d]);
    assert_eq!(shown["counts"]["colors"], 1);
    assert_eq!(shown["colors"][0]["name"], "primary");
    assert_eq!(shown["colors"][0]["resolved"]["value"], "#f59e0b");
    assert_eq!(shown["colors"][0]["resolved"]["status"], "resolved");
    assert_eq!(shown["type"][0]["resolved"]["value"]["font"], "Inter");
    assert_eq!(shown["inks"]["profiles"][0]["state"], "missing");
    // Re-applying is idempotent; show is deterministic.
    let again = ok(&ws, &["design", "brand", "apply", &text(&pkg), &d]);
    assert_eq!(again["would_change"], false);
    assert_eq!(shown, ok(&ws, &["design", "brand", "show", &d]));
}

#[test]
fn brand_apply_does_not_overwrite_a_local_brand_kit() {
    let ws = Workspace::new("brand-local");
    let (pkg, _) = brand_library(&ws, "1.0.0", brand_block(), &[]);
    let doc = document(&ws);
    let mut raw = read(&doc);
    raw["design"] = json!({"brand": {"colors": {}}});
    write(&doc, &raw);
    let before = fs::read(&doc).unwrap();
    let error = err(&ws, &["design", "brand", "apply", &text(&pkg), &text(&doc)]);
    assert_eq!(error["code"], "conflict");
    assert_eq!(before, fs::read(&doc).unwrap());
}

#[test]
fn brand_block_is_validated_at_pack_time() {
    let ws = Workspace::new("brand-invalid");
    let cases: [(Value, &str); 5] = [
        (
            json!({"colors":{"x":{"token":"color.missing"}}}),
            "unknown-token",
        ),
        (
            json!({"colors":{"x":{"token":"type.heading"}}}),
            "token-type-mismatch",
        ),
        (
            json!({"logos":[{"id":"a","asset":"../x"}]}),
            "invalid-brand",
        ),
        (
            json!({"rules":[{"id":"r","kind":"contrast","min":0.5}]}),
            "invalid-brand",
        ),
        (json!({"surprise":1}), "invalid-brand"),
    ];
    for (index, (brand, code)) in cases.into_iter().enumerate() {
        let (_, out) = brand_library(&ws, &format!("1.0.{index}"), brand, &[]);
        assert!(!out.status.success());
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["error"]["code"], code, "{error}");
    }
}

#[test]
fn restricted_profiles_block_packing_unless_allowed_locally() {
    let ws = Workspace::new("brand-profile");
    let mut brand = brand_block();
    brand["inks"]["profiles"][0]["redistribution"] = json!("restricted");
    let (_, out) = brand_library(&ws, "1.0.0", brand.clone(), &[]);
    assert!(!out.status.success());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["code"], "restricted-profile");
    let (_, out) = brand_library(&ws, "1.0.0", brand, &["--allow-restricted-local"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn brand_survives_library_update_flag_and_show_reports_profiles_present() {
    let ws = Workspace::new("brand-update");
    let (v1, _) = brand_library(&ws, "1.0.0", brand_block(), &[]);
    let doc = document(&ws);
    let d = text(&doc);
    ok(&ws, &["design", "brand", "apply", &text(&v1), &d]);
    let (v2, _) = brand_library(&ws, "1.1.0", brand_block(), &[]);
    ok(
        &ws,
        &["design", "library", "update", &d, "--package", &text(&v2)],
    );
    let raw = read(&doc);
    assert_eq!(raw["design"]["libraries"]["acme/brand"]["version"], "1.1.0");
    assert_eq!(raw["design"]["libraries"]["acme/brand"]["brand"], true);
    // A document brand that references a deleted token fails show with the token named.
    let mut raw = raw;
    raw["styles"]
        .as_object_mut()
        .unwrap()
        .remove("type.heading");
    write(&doc, &raw);
    let error = err(&ws, &["design", "brand", "show", &d]);
    assert_eq!(error["code"], "unknown-token");
    assert_eq!(error["token"], "type.heading");
}

/// A v4 template: headline text, accent bar, hidden badge and a locked layer.
fn template(ws: &Workspace) -> PathBuf {
    let path = ws.path("post.pen");
    quiet(ws, &["new", &text(&path)]);
    let mut doc = read(&path);
    doc["styles"] = json!({
        "color.accent": color("#f59e0b"),
        "color.brand": color("#2563eb"),
        "space.md": {"type":"spacing","value":16},
    });
    doc["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"text","id":"head","x":10,"y":30,"content":"Title","font_size":24,
         "style":{"fill":{"fallback":"#111827"}}},
        rect("bar", "color.accent"),
        {"kind":"rect","id":"badge","x":0,"y":0,"width":10,"height":10,"radius_x":0,"radius_y":0,
         "visible":false,"style":{"fill":{"fallback":"#ff0000"}}},
    ]);
    doc["pages"][0]["layers"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"frozen","name":"Frozen","visible":true,"locked":true,
            "nodes":[{"kind":"rect","id":"seal","x":0,"y":0,"width":4,"height":4,
                      "radius_x":0,"radius_y":0,"style":{"fill":{"fallback":"#000000"}}}]
        }));
    doc["design"] = json!({"template":{"schema":1,"name":"social/post","version":"1.0.0",
    "parameters":{
        "headline":{"type":"text","default":"Title","targets":[{"object":"head","property":"content"}],
                    "constraints":{"max_length":12,"required":true}},
        "accent":{"type":"token","token_type":"color","default":"color.accent",
                  "targets":[{"object":"bar","property":"fill"}]},
        "ink":{"type":"color","default":"#111827","targets":[{"object":"head","property":"fill"}],
               "constraints":{"enum":["#111827","#ffffff"]}},
        "show_badge":{"type":"bool","default":false,
                      "targets":[{"object":"badge","property":"visible"}]},
        "bar_width":{"type":"number","default":10,
                     "targets":[{"object":"bar","property":"width"}],
                     "constraints":{"min":1,"max":300}},
    }}});
    write(&path, &doc);
    path
}

fn values(ws: &Workspace, value: Value) -> String {
    let path = ws.path("values.json");
    write(&path, &value);
    text(&path)
}

#[test]
fn template_fill_dry_run_matches_commit() {
    let ws = Workspace::new("tfill");
    let t = template(&ws);
    let tpl = text(&t);
    let before = fs::read(&t).unwrap();
    quiet(&ws, &["design", "template", "check", &tpl]);
    let v = values(
        &ws,
        json!({"headline":"Launch day","accent":"color.brand","ink":"#ffffff",
               "show_badge":true,"bar_width":120}),
    );
    let out = ws.s("out.pen");
    let base = [
        "design", "template", "fill", &tpl, "--values", &v, "--output", &out,
    ];
    let mut dry_args = base.to_vec();
    dry_args.push("--dry-run");
    let dry = ok(&ws, &dry_args);
    assert!(!ws.path("out.pen").exists(), "dry run wrote the output");
    assert_eq!(dry["dry_run"], true);
    let commit = ok(&ws, &base);
    let mut left = dry.clone();
    let mut right = commit.clone();
    left["dry_run"] = json!(null);
    right["dry_run"] = json!(null);
    assert_eq!(left, right, "dry run and commit disagree");
    assert_eq!(commit["matches"], 5);
    assert_eq!(commit["parameters"]["headline"]["applied"], "Launch day");
    assert_eq!(fs::read(&t).unwrap(), before, "template was modified");

    let filled = read(&ws.path("out.pen"));
    let nodes = &filled["pages"][0]["layers"][0]["nodes"];
    assert_eq!(nodes[0]["content"], "Launch day");
    assert_eq!(nodes[0]["style"]["fill"]["fallback"], "#ffffff");
    assert_eq!(nodes[1]["style"]["fill"]["ref"], "color.brand");
    assert_eq!(nodes[1]["width"], 120.0);
    assert_eq!(nodes[2]["visible"], true);
    let from = &filled["design"]["template"]["filled_from"];
    assert_eq!(from["name"], "social/post");
    assert!(from["content_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(filled["design"]["template"]["values"]["bar_width"], 120);
    quiet(&ws, &["tree", &ws.s("out.pen"), "--limit", "5"]);

    // Defaults only: a repeated fill is deterministic.
    let empty = values(&ws, json!({}));
    let (a, b) = (ws.s("a.pen"), ws.s("b.pen"));
    let first = ok(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &empty, "--output", &a,
        ],
    );
    ok(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &empty, "--output", &b,
        ],
    );
    assert_eq!(
        fs::read(ws.path("a.pen")).unwrap(),
        fs::read(ws.path("b.pen")).unwrap()
    );
    assert_eq!(first["returned"], 5);
}

#[test]
fn template_fill_reports_all_errors() {
    let ws = Workspace::new("terrs");
    let t = template(&ws);
    let tpl = text(&t);
    let v = values(
        &ws,
        json!({"headline":"far too long for the limit","ink":"#123456",
               "bar_width":9000,"accent":"color.missing","nope":1}),
    );
    let out = ws.s("out.pen");
    let error = err(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &v, "--output", &out,
        ],
    );
    assert_eq!(error["code"], "invalid-input");
    let message = error["message"].as_str().unwrap();
    for needle in [
        "nope: unknown parameter",
        "headline: text is longer",
        "ink: value is not one of",
        "bar_width:",
        "accent: token \"color.missing\"",
    ] {
        assert!(message.contains(needle), "missing {needle:?} in {message}");
    }
    assert!(!ws.path("out.pen").exists());
}

#[test]
fn template_fill_never_modifies_template() {
    let ws = Workspace::new("tsafe");
    let t = template(&ws);
    let tpl = text(&t);
    let before = fs::read(&t).unwrap();
    let v = values(&ws, json!({"headline":"Hi"}));
    let out = ws.s("out.pen");
    let stale = err(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--values",
            &v,
            "--output",
            &out,
            "--if-revision",
            "deadbeef",
        ],
    );
    assert_eq!(stale["code"], "conflict");
    err(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &v, "--output", &tpl,
        ],
    );
    err(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &v, "--output", &out, "--page", "nope",
        ],
    );
    let limit = err(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &v, "--output", &out, "--limit", "0",
        ],
    );
    assert_eq!(limit["code"], "invalid-input");
    assert_eq!(fs::read(&t).unwrap(), before);
    assert!(!ws.path("out.pen").exists());

    let mut doc = read(&t);
    doc["design"]["template"]["parameters"]["lock"] = json!({"type":"color",
        "targets":[{"object":"seal","property":"fill"}]});
    doc["design"]["template"]["parameters"]["Bad-Name"] = json!({"type":"bool",
        "targets":[{"object":"ghost","property":"visible"}]});
    write(&t, &doc);
    let locked_before = fs::read(&t).unwrap();
    let error = err(&ws, &["design", "template", "check", &tpl]);
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("seal") && message.contains("locked"),
        "{message}"
    );
    assert!(
        message.contains("Bad-Name") && message.contains("ghost"),
        "{message}"
    );
    err(
        &ws,
        &[
            "design", "template", "fill", &tpl, "--values", &v, "--output", &out,
        ],
    );
    assert_eq!(fs::read(&t).unwrap(), locked_before);
    assert!(!ws.path("out.pen").exists());
}

#[test]
fn template_fill_changes_are_paginated_and_list_is_exact() {
    let ws = Workspace::new("tpage");
    let t = template(&ws);
    let tpl = text(&t);
    let v = values(&ws, json!({"headline":"Hi"}));
    let out = ws.s("out.pen");
    let page = ok(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--values",
            &v,
            "--output",
            &out,
            "--dry-run",
            "--offset",
            "3",
            "--limit",
            "1",
        ],
    );
    assert_eq!(page["matches"], 5);
    assert_eq!(page["returned"], 1);
    assert_eq!(page["offset"], 3);
    assert_eq!(page["has_more"], true);
    let dir = text(&ws.0);
    let listing = ok(&ws, &["design", "template", "list", &dir, "--limit", "1"]);
    assert_eq!(listing["matches"], 1);
    assert_eq!(listing["templates"][0]["name"], "social/post");
    assert_eq!(listing["templates"][0]["parameters"], 5);
    assert_eq!(listing["has_more"], false);
}

fn audit_document(ws: &Workspace) -> PathBuf {
    let path = ws.path("audit.pen");
    quiet(ws, &["new", &text(&path)]);
    let mut doc = read(&path);
    doc["styles"] = json!({
        "color.accent": color("#f59e0b"),
        "color.brand": color("#2563eb"),
        "color.white": color("#ffffff"),
        "color.pale": color("#eeeeee"),
        "type.body": {"type":"typography","value":{"font":"Inter","size":16,"weight":400}},
        "radius.card": {"type":"radius","value":8},
        "color.bad": {"type":"color","value":"nope"},
    });
    let fill = |id: &str, value: &str| {
        json!({"kind":"rect","id":id,"x":0,"y":0,"width":10,"height":10,"radius_x":0,"radius_y":0,
               "style":{"fill":{"fallback":value}}})
    };
    let mut nodes = vec![
        fill("exact", "#F59E0B"),
        fill("near", "#f59e0c"),
        fill("far", "#123456"),
        fill("clear", "none"),
        json!({"kind":"rect","id":"ghost","x":0,"y":0,"width":10,"height":10,"radius_x":4,"radius_y":4,
               "style":{"fill":{"ref":"color.missing","fallback":"#2563eb"}}}),
        json!({"kind":"rect","id":"broken","x":0,"y":0,"width":10,"height":10,"radius_x":8,"radius_y":8,
               "style":{"fill":{"ref":"color.bad","fallback":"#000000"}}}),
        json!({"kind":"text","id":"body","x":0,"y":0,"content":"Hi","font_family":"Inter",
               "font_size":16,"font_weight":400,"style":{"fill":{"ref":"color.pale","fallback":"#eeeeee"}}}),
        json!({"kind":"text","id":"odd","x":0,"y":20,"content":"Hi","font_family":"Comic",
               "font_size":13,"font_weight":400,"style":{"fill":{"ref":"color.brand","fallback":"#2563eb"}}}),
    ];
    for i in 0..60 {
        nodes.push(fill(&format!("bulk{i:02}"), "#123456"));
    }
    doc["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    doc["pages"][0]["layers"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "id":"frozen","name":"Frozen","visible":true,"locked":true,
            "nodes":[fill("sealed", "#2563eb")]
        }));
    doc["design"] = json!({"brand":{"schema":1,
        "rules":[{"id":"legible","kind":"contrast","min":4.5},
                 {"id":"fonts","kind":"fonts","allowed":["type.body"]}]}});
    write(&path, &doc);
    path
}

fn codes(report: &Value, code: &str) -> Vec<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == code)
        .map(|f| f["id"].as_str().or(f["token"].as_str()).unwrap().to_owned())
        .collect()
}

#[test]
fn audit_reports_hardcoded_unresolved_and_brand_findings() {
    let ws = Workspace::new("audit");
    let doc = audit_document(&ws);
    let path = text(&doc);
    let before = fs::read(&doc).unwrap();
    let report = ok(&ws, &["design", "audit", &path, "--limit", "500"]);
    assert_eq!(fs::read(&doc).unwrap(), before, "audit is read-only");

    let off = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["code"] == "off-token-color")
        .collect::<Vec<_>>();
    let exact = off.iter().find(|f| f["id"] == "exact").unwrap();
    assert_eq!(exact["suggest"], "color.accent");
    assert_eq!(exact["exact"], true);
    let near = off.iter().find(|f| f["id"] == "near").unwrap();
    assert_eq!(near["exact"], false);
    assert_eq!(near["suggest"], "color.accent");
    let far = off.iter().find(|f| f["id"] == "far").unwrap();
    assert!(far.get("suggest").is_none());
    assert!(off.iter().all(|f| f["id"] != "clear"));
    // An unresolved reference is reported and its fallback literal still audited.
    assert_eq!(codes(&report, "unresolved-token"), ["ghost"]);
    assert!(off.iter().any(|f| f["id"] == "ghost" && f["exact"] == true));
    assert_eq!(codes(&report, "invalid-token"), ["broken", "color.bad"]);
    // Font and radius.
    assert_eq!(codes(&report, "off-token-font"), ["odd"]);
    assert_eq!(codes(&report, "off-token-radius"), ["ghost"]);
    // Brand rules.
    assert_eq!(codes(&report, "brand-contrast"), ["body"]);
    assert_eq!(codes(&report, "brand-font"), ["odd"]);
    // Locked layer findings are flagged.
    let sealed = off.iter().find(|f| f["id"] == "sealed").unwrap();
    assert_eq!(sealed["locked"], true);
    // Unused tokens are info.
    assert!(codes(&report, "unused-token").contains(&"color.white".to_owned()));
    assert_eq!(report["severities"]["error"], 3);
    assert_eq!(
        report["matches"],
        report["findings"].as_array().unwrap().len()
    );
}

#[test]
fn audit_is_paginated_exactly_and_rule_filtered() {
    let ws = Workspace::new("auditpage");
    let path = text(&audit_document(&ws));
    let full = ok(&ws, &["design", "audit", &path, "--limit", "500"]);
    let total = full["matches"].as_u64().unwrap() as usize;
    assert!(total > 60);
    let first = ok(&ws, &["design", "audit", &path]);
    assert_eq!(first["limit"], 50);
    assert_eq!(first["returned"], 50);
    assert_eq!(first["has_more"], true);
    assert_eq!(first["matches"], total);
    assert_eq!(first["summary"], full["summary"]);
    let last = ok(
        &ws,
        &["design", "audit", &path, "--offset", "50", "--limit", "500"],
    );
    assert_eq!(last["returned"], total - 50);
    assert_eq!(last["has_more"], false);
    assert_eq!(
        first["findings"][0], full["findings"][0],
        "order is deterministic"
    );
    for args in [["--limit", "0"], ["--limit", "501"], ["--rules", "nope"]] {
        let mut command = vec!["design", "audit", &path];
        command.extend(args);
        assert_eq!(err(&ws, &command)["code"], "invalid-input");
    }
    err(&ws, &["design", "audit", &path, "--page", "ghost"]);
    let only = ok(
        &ws,
        &[
            "design", "audit", &path, "--rules", "refs", "--limit", "500",
        ],
    );
    assert!(only["summary"].get("off-token-color").is_none());
    assert!(only["summary"].get("unresolved-token").is_some());
}

#[test]
fn audit_fail_on_sets_exit_status_three() {
    let ws = Workspace::new("auditfail");
    let path = text(&audit_document(&ws));
    let out = run(&ws, &["design", "audit", &path, "--fail-on", "error"]);
    assert_eq!(out.status.code(), Some(3));
    let out = run(
        &ws,
        &[
            "design",
            "audit",
            &path,
            "--rules",
            "font",
            "--fail-on",
            "error",
        ],
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn audit_fix_plan_applies_exactly_through_batch() {
    let ws = Workspace::new("auditfix");
    let doc = audit_document(&ws);
    let path = text(&doc);
    let report = ok(
        &ws,
        &[
            "design",
            "audit",
            &path,
            "--rules",
            "color",
            "--fix-plan",
            "--limit",
            "500",
        ],
    );
    let plan = &report["plan"];
    let ops = plan["operations"].as_array().unwrap();
    let ids: Vec<&str> = ops.iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["exact", "ghost"],
        "exact matches on unlocked nodes: {plan}"
    );
    assert!(ops.iter().all(|o| o["type"] == "bind-token"));
    let skipped = plan["skipped"].as_array().unwrap();
    assert!(skipped
        .iter()
        .any(|s| s["id"] == "sealed" && s["reason"] == "locked"));
    assert!(skipped.iter().any(|s| s["id"] == "near"));

    let ops_file = ws.path("plan.json");
    write(&ops_file, &json!(ops));
    let before = fs::read(&doc).unwrap();
    quiet(&ws, &["batch", &path, &text(&ops_file), "--dry-run"]);
    assert_eq!(fs::read(&doc).unwrap(), before);
    quiet(&ws, &["batch", &path, &text(&ops_file)]);

    let after = read(&doc);
    let mut expected = serde_json::from_slice::<Value>(&before).unwrap();
    for (id, token, hex) in [
        ("exact", "color.accent", "#f59e0b"),
        ("ghost", "color.brand", "#2563eb"),
    ] {
        let node = expected["pages"][0]["layers"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|n| n["id"] == id)
            .unwrap();
        node["style"]["fill"] = json!({"ref":token,"fallback":hex});
    }
    assert_eq!(
        after["pages"], expected["pages"],
        "only listed nodes change"
    );
    let again = ok(
        &ws,
        &[
            "design", "audit", &path, "--rules", "color", "--limit", "500",
        ],
    );
    let exact_left = again["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["exact"] == true && f["locked"].is_null())
        .count();
    assert_eq!(exact_left, 0);

    let nearest = ok(
        &ws,
        &[
            "design",
            "audit",
            &path,
            "--rules",
            "color",
            "--fix-plan",
            "--fix-nearest",
            "--limit",
            "500",
        ],
    );
    assert!(nearest["plan"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["id"] == "near"));
}

#[test]
fn bind_token_and_set_token_batch_ops_respect_locks_and_validity() {
    let ws = Workspace::new("bindops");
    let doc = audit_document(&ws);
    let path = text(&doc);
    let ops = ws.path("ops.json");
    let before = fs::read(&doc).unwrap();
    for (op, message) in [
        (
            json!({"type":"bind-token","id":"sealed","property":"fill","token":"color.accent"}),
            "locked",
        ),
        (
            json!({"type":"bind-token","id":"far","property":"fill","token":"color.missing"}),
            "unresolved",
        ),
        (
            json!({"type":"bind-token","id":"far","property":"fill","token":"radius.card"}),
            "color",
        ),
        (
            json!({"type":"bind-token","id":"far","property":"width","token":"color.accent"}),
            "fill and stroke",
        ),
        (
            json!({"type":"set-token","name":"color.new","token_type":"color","value":"nope"}),
            "not a color",
        ),
        (
            json!({"type":"set-token","name":"space.x","token_type":"bogus","value":1}),
            "unknown token type",
        ),
    ] {
        write(&ops, &json!([op]));
        let out = run(&ws, &["batch", &path, &text(&ops)]);
        assert!(!out.status.success(), "{op}");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(stderr.contains(message), "{op}: {stderr}");
        assert_eq!(fs::read(&doc).unwrap(), before, "failed batch leaves bytes");
    }
    write(
        &ops,
        &json!([
            {"type":"set-token","name":"space.md","token_type":"spacing","value":16},
            {"type":"set-token","name":"color.cta","token_type":"color","value":{"ref":"color.brand"}},
            {"type":"bind-token","id":"far","property":"stroke","token":"color.cta"},
        ]),
    );
    quiet(&ws, &["batch", &path, &text(&ops)]);
    let after = read(&doc);
    assert_eq!(
        after["styles"]["space.md"],
        json!({"type":"spacing","value":16})
    );
    let far = after["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "far")
        .unwrap();
    assert_eq!(
        far["style"]["stroke"],
        json!({"ref":"color.cta","fallback":"#2563eb"})
    );
}

// ---------------------------------------------------------------- item 8 --

fn native_tokens() -> Value {
    json!({"schema":1,"tokens":{
        "color.accent":{"type":"color","value":"#F59E0B","description":"CTA"},
        "color.cta":{"type":"color","value":{"ref":"color.accent"}},
        "radius.card":{"type":"radius","value":8},
        "shadow.raised":{"type":"shadow","value":[
            {"dx":0,"dy":2,"blur":8,"spread":0,"color":"#00000033"}]},
        "space.md":{"type":"spacing","value":16},
        "space.tight":{"type":"spacing","value":0.5},
        "type.body":{"type":"typography",
            "value":{"font":"Inter","size":16,"weight":400,"leading":24,"tracking":-0.25}}
    }})
}

fn new_doc(ws: &Workspace, name: &str) -> PathBuf {
    let path = ws.path(name);
    quiet(ws, &["new", &text(&path)]);
    path
}

fn export_text(ws: &Workspace, doc: &Path, format: &str) -> String {
    let out = run(
        ws,
        &["design", "token", "export", &text(doc), "--format", format],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn import_args<'a>(doc: &'a str, file: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["design", "token", "import", doc, file];
    args.extend_from_slice(extra);
    args
}

#[test]
fn token_file_native_roundtrip_is_lossless_and_dry_run_matches_commit() {
    let ws = Workspace::new("tf-native");
    let file = ws.path("tokens.json");
    // The deterministic export form: sorted IDs, 2-space indent, trailing newline.
    let canonical = format!(
        "{}\n",
        serde_json::to_string_pretty(&native_tokens()).unwrap()
    );
    fs::write(&file, &canonical).unwrap();
    let doc = new_doc(&ws, "a.pen");
    let (d, f) = (text(&doc), text(&file));
    let before = fs::read(&doc).unwrap();
    let dry = ok(&ws, &import_args(&d, &f, &["--dry-run"]));
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["would_change"], true);
    assert_eq!(dry["counts"]["added"], 7);
    assert_eq!(fs::read(&doc).unwrap(), before, "dry run writes nothing");
    let real = ok(&ws, &import_args(&d, &f, &[]));
    for key in [
        "counts",
        "imported",
        "skipped",
        "writes",
        "tokens",
        "conflicts",
    ] {
        assert_eq!(dry[key], real[key], "{key}");
    }
    assert_ne!(fs::read(&doc).unwrap(), before);
    assert_eq!(export_text(&ws, &doc, "native"), canonical);
    // Re-import is a no-op that does not touch the file.
    let again_before = fs::read(&doc).unwrap();
    let again = ok(&ws, &import_args(&d, &f, &[]));
    assert_eq!(again["would_change"], false);
    assert_eq!(again["counts"]["unchanged"], 7);
    assert_eq!(fs::read(&doc).unwrap(), again_before);
    // --output writes the same bytes.
    let out = ws.path("out.json");
    let report = ok(
        &ws,
        &["design", "token", "export", &d, "--output", &text(&out)],
    );
    assert_eq!(report["tokens"], 7);
    assert_eq!(fs::read_to_string(&out).unwrap(), canonical);
    // Undo restores the pre-import bytes.
    let doc2 = new_doc(&ws, "b.pen");
    let original = fs::read(&doc2).unwrap();
    quiet(&ws, &import_args(&text(&doc2), &f, &[]));
    quiet(&ws, &["undo", &text(&doc2)]);
    assert_eq!(fs::read(&doc2).unwrap(), original);
}

#[test]
fn token_import_never_overwrites_local_tokens_silently() {
    let ws = Workspace::new("tf-conflict");
    let doc = new_doc(&ws, "a.pen");
    let d = text(&doc);
    let local = ws.path("local.json");
    write(
        &local,
        &json!({"schema":1,"tokens":{
            "color.accent":{"type":"color","value":"#111111"}}}),
    );
    quiet(&ws, &import_args(&d, &text(&local), &[]));
    let file = ws.path("tokens.json");
    write(&file, &native_tokens());
    let f = text(&file);
    let before = fs::read(&doc).unwrap();
    // Default fail: structured conflict error, bytes unchanged.
    let error = err(&ws, &import_args(&d, &f, &[]));
    assert_eq!(error["code"], "conflict");
    assert!(error["message"].as_str().unwrap().contains("color.accent"));
    assert_eq!(fs::read(&doc).unwrap(), before);
    // The dry run reports the conflict (values on both sides) without failing.
    let dry = ok(&ws, &import_args(&d, &f, &["--dry-run"]));
    assert_eq!(dry["blocked"], true);
    assert_eq!(dry["writes"], json!([]));
    assert_eq!(dry["conflicts"][0]["token"], "color.accent");
    let row = dry["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["token"] == "color.accent")
        .unwrap();
    assert_eq!(row["local"]["value"], "#111111");
    assert_eq!(row["incoming"]["value"], "#F59E0B");
    // skip keeps the local value and imports the rest.
    let skipped = ok(&ws, &import_args(&d, &f, &["--on-collision", "skip"]));
    assert_eq!(skipped["skipped"], json!(["color.accent"]));
    assert_eq!(read(&doc)["styles"]["color.accent"]["value"], "#111111");
    assert_eq!(read(&doc)["styles"]["space.md"]["value"], 16);
    // overwrite replaces and reports it.
    let over = ok(&ws, &import_args(&d, &f, &["--on-collision", "overwrite"]));
    assert_eq!(over["counts"]["overwritten"], 1);
    assert_eq!(read(&doc)["styles"]["color.accent"]["value"], "#F59E0B");
    // A stale revision guard is refused.
    let stale = err(&ws, &import_args(&d, &f, &["--if-revision", "sha256:nope"]));
    assert_eq!(stale["code"], "conflict");
}

#[test]
fn token_import_validates_before_any_write() {
    let ws = Workspace::new("tf-invalid");
    let doc = new_doc(&ws, "a.pen");
    let (d, f) = (text(&doc), ws.s("bad.json"));
    let file = ws.path("bad.json");
    let before = fs::read(&doc).unwrap();
    let cases = [
        (
            json!({"schema":1,"tokens":{
                "color.a":{"type":"color","value":{"ref":"color.b"}},
                "color.b":{"type":"color","value":{"ref":"color.a"}}}}),
            "token-cycle",
        ),
        (
            json!({"schema":1,"tokens":{"Color.Bad":{"type":"color","value":"#fff"}}}),
            "invalid-token-id",
        ),
        (
            json!({"schema":1,"tokens":{"color.a":{"type":"color","value":"not-a-color"}}}),
            "invalid-token-value",
        ),
        (
            json!({"schema":1,"tokens":{
                "color.a":{"type":"color","value":"#fff"},
                "space.a":{"type":"spacing","value":{"ref":"color.a"}}}}),
            "token-type-mismatch",
        ),
        (
            json!({"schema":1,"tokens":{"color.a":{"type":"color","value":{"ref":"color.zzz"}}}}),
            "unknown-token",
        ),
        (json!({"schema":2,"tokens":{}}), "invalid-input"),
    ];
    for (body, code) in cases {
        write(&file, &body);
        let error = err(&ws, &import_args(&d, &f, &[]));
        assert_eq!(error["code"], code, "{body}");
        assert_eq!(fs::read(&doc).unwrap(), before, "{code} leaves bytes");
    }
    // Token count limit.
    let mut many = serde_json::Map::new();
    for index in 0..5_001 {
        many.insert(
            format!("space.s{index}"),
            json!({"type":"spacing","value":1}),
        );
    }
    write(&file, &json!({"schema":1,"tokens":many}));
    let error = err(&ws, &import_args(&d, &f, &[]));
    assert_eq!(error["code"], "limit-exceeded");
    // The document limit counts existing tokens too.
    let mut fits = serde_json::Map::new();
    for index in 0..4_000 {
        fits.insert(
            format!("space.s{index}"),
            json!({"type":"spacing","value":1}),
        );
    }
    write(&file, &json!({"schema":1,"tokens":fits}));
    quiet(&ws, &import_args(&d, &f, &[]));
    let after = fs::read(&doc).unwrap();
    let mut more = serde_json::Map::new();
    for index in 0..1_001 {
        more.insert(
            format!("radius.r{index}"),
            json!({"type":"radius","value":1}),
        );
    }
    write(&file, &json!({"schema":1,"tokens":more}));
    let error = err(&ws, &import_args(&d, &f, &[]));
    assert_eq!(error["code"], "limit-exceeded");
    assert_eq!(fs::read(&doc).unwrap(), after);
}

#[test]
fn w3c_roundtrip_subset() {
    let ws = Workspace::new("tf-w3c");
    let native = ws.path("native.json");
    write(&native, &native_tokens());
    let a = new_doc(&ws, "a.pen");
    quiet(&ws, &import_args(&text(&a), &text(&native), &[]));
    let w3c = export_text(&ws, &a, "w3c");
    let parsed: Value = serde_json::from_str(&w3c).unwrap();
    assert_eq!(parsed["color"]["accent"]["$type"], "color");
    assert_eq!(parsed["color"]["cta"]["$value"], "{color.accent}");
    assert_eq!(parsed["space"]["md"]["$value"], "16px");
    assert_eq!(parsed["space"]["tight"]["$value"], "0.5px");
    assert_eq!(parsed["type"]["body"]["$value"]["fontFamily"], "Inter");
    assert_eq!(parsed["type"]["body"]["$value"]["letterSpacing"], "-0.25px");
    assert_eq!(parsed["shadow"]["raised"]["$value"][0]["offsetY"], "2px");
    assert_eq!(parsed["color"]["accent"]["$description"], "CTA");
    // w3c -> native -> w3c is byte-identical, and the native result matches.
    let file = ws.path("tokens.w3c.json");
    fs::write(&file, &w3c).unwrap();
    let b = new_doc(&ws, "b.pen");
    let dry = ok(&ws, &import_args(&text(&b), &text(&file), &["--dry-run"]));
    assert_eq!(dry["format"], "w3c");
    quiet(&ws, &import_args(&text(&b), &text(&file), &[]));
    assert_eq!(export_text(&ws, &b, "w3c"), w3c);
    assert_eq!(
        export_text(&ws, &b, "native"),
        export_text(&ws, &a, "native")
    );
    // --format w3c forces the format.
    let c = new_doc(&ws, "c.pen");
    quiet(
        &ws,
        &import_args(&text(&c), &text(&file), &["--format", "w3c"]),
    );
}

#[test]
fn w3c_rejections() {
    let ws = Workspace::new("tf-reject");
    let doc = new_doc(&ws, "a.pen");
    let (d, f) = (text(&doc), ws.s("t.json"));
    let file = ws.path("t.json");
    let before = fs::read(&doc).unwrap();
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"color":{"a":{"$type":"color","$value":"#fff","$extensions":{"x":1}}}}),
            "/color/a/$extensions",
        ),
        (
            json!({"color":{"a":{"$type":"color","$value":"#fff","$deprecated":true}}}),
            "/color/a/$deprecated",
        ),
        (
            json!({"color":{"$root":{"$type":"color","$value":"#fff"}}}),
            "/color/$root",
        ),
        (
            json!({"space":{"a":{"$type":"dimension","$value":"1rem"}}}),
            "/space/a/$value",
        ),
        (
            json!({"space":{"a":{"$type":"dimension","$value":{"value":1,"unit":"px"}}}}),
            "/space/a/$value",
        ),
        (
            json!({"dur":{"a":{"$type":"duration","$value":"100ms"}}}),
            "/dur/a",
        ),
        (
            json!({"grad":{"a":{"$type":"gradient","$value":[]}}}),
            "/grad/a",
        ),
        (
            json!({"color":{"a":{"$type":"color",
                "$value":{"colorSpace":"display-p3","components":[1,0,0]}}}}),
            "/color/a/$value",
        ),
        (
            json!({"type":{"a":{"$type":"typography","$value":{
                "fontFamily":"Inter","fontSize":"16px","lineHeight":1.5}}}}),
            "/type/a/$value/lineHeight",
        ),
        (
            json!({"type":{"a":{"$type":"typography","$value":{
                "fontFamily":["Inter","Arial"],"fontSize":"16px"}}}}),
            "/type/a/$value/fontFamily",
        ),
        (
            json!({"type":{"a":{"$type":"typography","$value":{
                "fontFamily":"Inter","fontSize":"{space.md}"}}}}),
            "/type/a/$value/fontSize",
        ),
        (
            json!({"shadow":{"a":{"$type":"shadow","$value":{
                "offsetX":"0px","offsetY":"1px","blur":"2px","spread":"0px",
                "color":"#000","inset":true}}}}),
            "/shadow/a/$value/inset",
        ),
        (
            json!({"font":{"a":{"$type":"fontFamily","$value":"Inter"}}}),
            "/font/a",
        ),
        (
            json!({"misc":{"a":{"$type":"dimension","$value":"4px"}}}),
            "/misc/a",
        ),
        (
            json!({"color":{"a":{"$type":"color","$value":"#fff","$ref":"#/x"}}}),
            "/color/a/$ref",
        ),
    ];
    for (body, pointer) in cases {
        write(&file, &body);
        let error = err(&ws, &import_args(&d, &f, &["--format", "w3c"]));
        assert_eq!(error["code"], "unsupported-token-construct", "{body}");
        assert_eq!(error["path"], pointer, "{body}");
        assert_eq!(fs::read(&doc).unwrap(), before);
    }
}

#[test]
fn token_export_refuses_or_lists_what_does_not_map() {
    let ws = Workspace::new("tf-unmappable");
    let doc = new_doc(&ws, "a.pen");
    let mut value = read(&doc);
    value["styles"] = json!({
        "color.ok": {"type":"color","value":"#112233"},
        "color.alias": {"type":"color","value":{"ref":"color.none"}},
        "color.none": {"type":"color","value":"none"},
        "radius.pair": {"type":"radius","value":[2,3]},
        "gap.x": {"type":"spacing","value":4},
        "Legacy Name": {"type":"color","value":"#000000"},
    });
    write(&doc, &value);
    // Native export rejects the legacy ID.
    let error = err(&ws, &["design", "token", "export", &text(&doc)]);
    assert_eq!(error["code"], "invalid-token-id");
    let out = ws.path("out.json");
    let report = ok(
        &ws,
        &[
            "design",
            "token",
            "export",
            &text(&doc),
            "--format",
            "w3c",
            "--skip-unmappable",
            "--output",
            &text(&out),
        ],
    );
    assert_eq!(report["tokens"], 1);
    let skipped: Vec<&str> = report["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["token"].as_str().unwrap())
        .collect();
    assert_eq!(
        skipped,
        vec![
            "Legacy Name",
            "color.alias",
            "color.none",
            "gap.x",
            "radius.pair"
        ]
    );
    assert_eq!(read(&out)["color"]["ok"]["$value"], "#112233");
}

#[test]
fn library_export_writes_the_package_tokens() {
    let ws = Workspace::new("tf-library");
    let package = library(&ws, "1.0.0", base_tokens());
    let out = ws.path("lib.json");
    let report = ok(
        &ws,
        &[
            "design",
            "library",
            "export",
            &text(&package),
            "--output",
            &text(&out),
        ],
    );
    assert_eq!(report["tokens"], 23);
    let exported = read(&out);
    assert_eq!(exported["schema"], 1);
    assert_eq!(exported["tokens"]["color.accent"]["value"], "#f59e0b");
    assert_eq!(
        exported["tokens"]["color.cta"]["value"],
        json!({"ref":"color.accent"})
    );
    // The exported file imports into a document.
    let doc = new_doc(&ws, "a.pen");
    quiet(&ws, &import_args(&text(&doc), &text(&out), &[]));
    assert_eq!(read(&doc)["styles"].as_object().unwrap().len(), 23);
    // The library's tokens include a 20-color ramp, so W3C export maps them too.
    let w3c = run(
        &ws,
        &[
            "design",
            "library",
            "export",
            &text(&package),
            "--format",
            "w3c",
        ],
    );
    assert!(
        w3c.status.success(),
        "{}",
        String::from_utf8_lossy(&w3c.stderr)
    );
}

fn out_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn template_fill_data_json_and_csv_rows() {
    let ws = Workspace::new("tdata");
    let t = template(&ws);
    let tpl = text(&t);
    let before = fs::read(&t).unwrap();
    let rows = ws.path("rows.json");
    write(
        &rows,
        &json!([
            {"headline":"One","bar_width":40},
            {"headline":"Two","show_badge":true},
            {}
        ]),
    );
    let (rows_s, dir) = (text(&rows), ws.s("out"));
    let base = [
        "design",
        "template",
        "fill",
        &tpl,
        "--data",
        &rows_s,
        "--output-dir",
        &dir,
    ];
    let mut dry_args = base.to_vec();
    dry_args.push("--dry-run");
    let dry = ok(&ws, &dry_args);
    assert!(!ws.path("out").exists(), "dry run wrote output");
    let commit = ok(&ws, &base);
    let (mut l, mut r) = (dry.clone(), commit.clone());
    l["dry_run"] = json!(null);
    r["dry_run"] = json!(null);
    assert_eq!(l, r, "dry run and commit disagree");
    assert_eq!(commit["rows"], 3);
    assert_eq!(
        out_files(&ws.path("out")),
        ["post-00001.pen", "post-00002.pen", "post-00003.pen"]
    );
    let first = read(&ws.path("out").join("post-00001.pen"));
    assert_eq!(first["pages"][0]["layers"][0]["nodes"][0]["content"], "One");
    assert_eq!(first["pages"][0]["layers"][0]["nodes"][1]["width"], 40.0);
    let second = read(&ws.path("out").join("post-00002.pen"));
    assert_eq!(second["pages"][0]["layers"][0]["nodes"][2]["visible"], true);
    assert_eq!(fs::read(&t).unwrap(), before, "template was modified");

    // Re-running the identical fill is a no-op; different content never overwrites.
    let again = ok(&ws, &base);
    assert_eq!(again["would_change"], false);
    write(&rows, &json!([{"headline":"Changed"}]));
    let error = err(&ws, &base);
    assert_eq!(error["code"], "conflict");
    assert_eq!(out_files(&ws.path("out")).len(), 3);

    // CSV: header of parameter names, cells coerced by type, empty = default.
    let csv = ws.path("rows.csv");
    fs::write(
        &csv,
        "headline,bar_width,show_badge\n\"Hello, world\",25,true\nPlain,,\n",
    )
    .unwrap();
    let (csv_s, dir2) = (text(&csv), ws.s("out2"));
    let report = ok(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &csv_s,
            "--output-dir",
            &dir2,
            "--limit",
            "1",
        ],
    );
    assert_eq!(report["rows"], 2);
    assert_eq!(report["returned"], 1);
    assert_eq!(report["has_more"], true);
    let one = read(&ws.path("out2").join("post-00001.pen"));
    assert_eq!(
        one["pages"][0]["layers"][0]["nodes"][0]["content"],
        "Hello, world"
    );
    assert_eq!(one["pages"][0]["layers"][0]["nodes"][1]["width"], 25.0);
    assert_eq!(one["pages"][0]["layers"][0]["nodes"][2]["visible"], true);
}

#[test]
fn template_fill_data_validates_every_row_before_writing() {
    let ws = Workspace::new("tdatabad");
    let t = template(&ws);
    let tpl = text(&t);
    let rows = ws.path("rows.json");
    write(
        &rows,
        &json!([
            {"headline":"fine"},
            {"headline":"this headline is far too long"},
            {"bar_width":9000, "nope":1}
        ]),
    );
    let (rows_s, dir) = (text(&rows), ws.s("out"));
    let error = err(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &rows_s,
            "--output-dir",
            &dir,
        ],
    );
    assert_eq!(error["code"], "invalid-input");
    let message = error["message"].as_str().unwrap();
    for needle in [
        "row 2: headline",
        "row 3: bar_width",
        "row 3: nope: unknown",
    ] {
        assert!(message.contains(needle), "missing {needle:?} in {message}");
    }
    assert!(!ws.path("out").exists(), "a bad row must write nothing");

    // Bad CSV cell.
    let csv = ws.path("bad.csv");
    fs::write(&csv, "headline,bar_width\nA,wide\n").unwrap();
    let csv_s = text(&csv);
    let error = err(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &csv_s,
            "--output-dir",
            &dir,
        ],
    );
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("row 1: bar_width"));
    assert!(!ws.path("out").exists());

    // Mixing the single-fill flags with --data is rejected.
    let v = values(&ws, json!({}));
    let out = ws.s("o.pen");
    assert!(!run(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &rows_s,
            "--output-dir",
            &dir,
            "--values",
            &v,
            "--output",
            &out,
        ],
    )
    .status
    .success());
}

#[test]
fn template_fill_data_enforces_the_row_limit() {
    let ws = Workspace::new("tdatalimit");
    let t = template(&ws);
    let tpl = text(&t);
    let rows = ws.path("rows.json");
    write(&rows, &Value::Array(vec![json!({}); 10_001]));
    let (rows_s, dir) = (text(&rows), ws.s("out"));
    let error = err(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &rows_s,
            "--output-dir",
            &dir,
            "--dry-run",
        ],
    );
    assert_eq!(error["code"], "limit-exceeded");
    assert!(!ws.path("out").exists());
    // Exactly at the limit is accepted (dry run).
    write(&rows, &Value::Array(vec![json!({}); 10_000]));
    let report = ok(
        &ws,
        &[
            "design",
            "template",
            "fill",
            &tpl,
            "--data",
            &rows_s,
            "--output-dir",
            &dir,
            "--dry-run",
            "--limit",
            "2",
        ],
    );
    assert_eq!(report["rows"], 10_000);
    assert_eq!(report["returned"], 2);
}
