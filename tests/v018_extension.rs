//! v0.18.0 sandboxed extensions: packaging, approvals, the document API through
//! the shared transaction path, and a hostile-module suite.
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
            "pentool-ext-{tag}-{}-{}",
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
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pentool(ws: &Workspace, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pentool"))
        .current_dir(&ws.0)
        .args(args)
        .output()
        .unwrap()
}

fn ok(ws: &Workspace, args: &[&str]) -> Value {
    let out = pentool(ws, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?} printed non-JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn quiet(ws: &Workspace, args: &[&str]) {
    let out = pentool(ws, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn fails(ws: &Workspace, args: &[&str]) -> String {
    let out = pentool(ws, args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn wat_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A guest that sends each request in order (ignoring responses) and returns `status`.
fn guest(requests: &[Value], status: i32) -> Vec<u8> {
    let mut data = String::new();
    let mut calls = String::new();
    let mut offset = 0;
    for request in requests {
        let text = request.to_string();
        data += &format!("(data (i32.const {offset}) \"{}\")\n", wat_escape(&text));
        calls += &format!(
            "(drop (call $call (i32.const {offset}) (i32.const {}) (i32.const 65536) (i32.const 65536)))\n",
            text.len()
        );
        offset += text.len();
    }
    wat::parse_str(format!(
        r#"(module
  (import "pentool" "call" (func $call (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 3)
  {data}
  (func (export "run") (result i32) {calls} (i32.const {status})))"#
    ))
    .unwrap()
}

fn manifest(name: &str, caps: &[&str], limits: Value) -> Value {
    json!({
        "name": name, "version": "1.0.0", "api": 1,
        "capabilities": caps, "entry": "main.wasm", "limits": limits,
        "commands": [{"id": "go", "title": "Go"}],
    })
}

/// Pack `wasm` into `<name>.penext` in the workspace and return its path.
fn build(ws: &Workspace, name: &str, caps: &[&str], limits: Value, wasm: &[u8]) -> PathBuf {
    let dir = ws.path(&format!("src-{name}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("extension.json"),
        manifest(name, caps, limits).to_string(),
    )
    .unwrap();
    fs::write(dir.join("main.wasm"), wasm).unwrap();
    let out = ws.path(&format!("{name}.penext"));
    ok(
        ws,
        &[
            "extension",
            "pack",
            dir.to_str().unwrap(),
            out.to_str().unwrap(),
        ],
    );
    out
}

fn install(ws: &Workspace, package: &Path) {
    let report = ok(
        ws,
        &["extension", "add", package.to_str().unwrap(), "--approve"],
    );
    assert_eq!(report["installed"], true);
}

fn new_doc(ws: &Workspace) -> PathBuf {
    let doc = ws.path("doc.pen");
    quiet(
        ws,
        &[
            "new",
            doc.to_str().unwrap(),
            "--width",
            "400",
            "--height",
            "300",
        ],
    );
    doc
}

fn add_box_op(id: &str) -> Value {
    json!({"type":"put-shape","shape":"rect","id":id,"layer":"layer-1",
           "x":10,"y":10,"width":50,"height":40,"fill":"#ff0000"})
}

fn apply_request(ops: Vec<Value>) -> Value {
    json!({"method":"document.apply","params":{"operations":ops}})
}

const EDIT: &[&str] = &["document.read", "document.edit"];

#[test]
fn pack_is_deterministic_and_add_requires_explicit_approval() {
    let ws = Workspace::new("pack");
    let wasm = guest(&[], 0);
    let a = build(&ws, "det", EDIT, json!({}), &wasm);
    let first = fs::read(&a).unwrap();
    let b = build(&ws, "det", EDIT, json!({}), &wasm);
    assert_eq!(first, fs::read(&b).unwrap(), "same inputs, same bytes");

    let verified = ok(&ws, &["extension", "verify", a.to_str().unwrap()]);
    assert_eq!(verified["name"], "det");
    assert!(verified["package_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let prompt = ok(&ws, &["extension", "add", a.to_str().unwrap()]);
    assert_eq!(prompt["approval_required"], true);
    assert_eq!(prompt["installed"], false);
    assert_eq!(prompt["extension"]["capabilities"], json!(EDIT));
    assert_eq!(ok(&ws, &["extension", "list"])["count"], 0);

    install(&ws, &a);
    let listed = ok(&ws, &["extension", "list"]);
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["extensions"][0]["status"], "ok");
    assert_eq!(listed["extensions"][0]["capabilities"], json!(EDIT));
    ok(&ws, &["extension", "remove", "det"]);
    assert_eq!(ok(&ws, &["extension", "list"])["count"], 0);
    assert!(fails(&ws, &["extension", "remove", "det"]).contains("not installed"));
}

#[test]
fn run_edits_through_the_shared_transaction_and_is_undoable() {
    let ws = Workspace::new("edit");
    let doc = new_doc(&ws);
    let wasm = guest(&[apply_request(vec![add_box_op("ext-box")])], 0);
    install(&ws, &build(&ws, "adder", EDIT, json!({}), &wasm));

    let before = fs::read(&doc).unwrap();
    let report = ok(
        &ws,
        &[
            "extension",
            "run",
            "adder",
            doc.to_str().unwrap(),
            "--command",
            "go",
        ],
    );
    assert_eq!(report["changed"], true);
    assert_eq!(report["calls"], 1);
    assert_eq!(report["change"]["operation"], "extension:adder");
    assert_ne!(
        report["change"]["revision_before"],
        report["change"]["revision_after"]
    );
    let tree = ok(&ws, &["search", doc.to_str().unwrap(), "ext-box"]);
    assert!(tree.to_string().contains("ext-box"));

    quiet(&ws, &["undo", doc.to_str().unwrap()]);
    assert_eq!(
        fs::read(&doc).unwrap(),
        before,
        "one extension run, one undo"
    );
}

#[test]
fn dry_run_and_revision_guard_leave_the_document_untouched() {
    let ws = Workspace::new("guard");
    let doc = new_doc(&ws);
    let wasm = guest(&[apply_request(vec![add_box_op("planned")])], 0);
    install(&ws, &build(&ws, "planner", EDIT, json!({}), &wasm));
    let before = fs::read(&doc).unwrap();

    let dry = ok(
        &ws,
        &[
            "extension",
            "run",
            "planner",
            doc.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["changed"], true, "dry run reports the same plan");
    assert_eq!(fs::read(&doc).unwrap(), before);

    let stderr = fails(
        &ws,
        &[
            "extension",
            "run",
            "planner",
            doc.to_str().unwrap(),
            "--if-revision",
            "deadbeef",
        ],
    );
    assert!(stderr.contains("revision mismatch"), "{stderr}");
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn failed_or_nonzero_runs_never_change_the_document() {
    let ws = Workspace::new("rollback");
    let doc = new_doc(&ws);
    let before = fs::read(&doc).unwrap();
    // A valid edit followed by a non-zero exit status is discarded.
    let wasm = guest(&[apply_request(vec![add_box_op("lost")])], 7);
    install(&ws, &build(&ws, "quitter", EDIT, json!({}), &wasm));
    let stderr = fails(&ws, &["extension", "run", "quitter", doc.to_str().unwrap()]);
    assert!(stderr.contains("status 7"), "{stderr}");
    assert_eq!(fs::read(&doc).unwrap(), before);

    // A batch with one bad operation is rejected whole and leaves nothing behind.
    let mut bad = add_box_op("half");
    bad["layer"] = json!("no-such-layer");
    let wasm = guest(&[apply_request(vec![add_box_op("first"), bad])], 0);
    install(&ws, &build(&ws, "halfway", EDIT, json!({}), &wasm));
    let report = ok(&ws, &["extension", "run", "halfway", doc.to_str().unwrap()]);
    assert_eq!(report["changed"], false);
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn locks_and_read_only_capabilities_are_enforced() {
    let ws = Workspace::new("locks");
    let doc = new_doc(&ws);
    quiet(
        &ws,
        &[
            "layer",
            doc.to_str().unwrap(),
            "set",
            "layer-1",
            "--locked",
            "true",
        ],
    );
    let before = fs::read(&doc).unwrap();
    let wasm = guest(&[apply_request(vec![add_box_op("blocked")])], 0);
    install(&ws, &build(&ws, "locked", EDIT, json!({}), &wasm));
    let report = ok(&ws, &["extension", "run", "locked", doc.to_str().unwrap()]);
    assert_eq!(
        report["changed"], false,
        "a locked layer rejects extension edits"
    );
    assert_eq!(fs::read(&doc).unwrap(), before);

    quiet(
        &ws,
        &[
            "layer",
            doc.to_str().unwrap(),
            "set",
            "layer-1",
            "--locked",
            "false",
        ],
    );
    let before = fs::read(&doc).unwrap();
    let wasm = guest(&[apply_request(vec![add_box_op("denied")])], 0);
    install(
        &ws,
        &build(&ws, "readonly", &["document.read"], json!({}), &wasm),
    );
    let report = ok(
        &ws,
        &["extension", "run", "readonly", doc.to_str().unwrap()],
    );
    assert_eq!(report["changed"], false, "document.edit was never granted");
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn runaway_cpu_is_stopped_and_the_document_is_unchanged() {
    let ws = Workspace::new("cpu");
    let doc = new_doc(&ws);
    let before = fs::read(&doc).unwrap();
    let wasm = wat::parse_str(
        r#"(module (memory (export "memory") 1)
             (func (export "run") (result i32) (loop $l (br $l)) (i32.const 0)))"#,
    )
    .unwrap();
    install(&ws, &build(&ws, "spin", EDIT, json!({"cpu_ms": 20}), &wasm));
    let started = std::time::Instant::now();
    let stderr = fails(&ws, &["extension", "run", "spin", doc.to_str().unwrap()]);
    assert!(stderr.contains("cpu budget"), "{stderr}");
    assert!(started.elapsed().as_secs() < 30);
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn memory_exhaustion_traps() {
    let ws = Workspace::new("mem");
    let doc = new_doc(&ws);
    let before = fs::read(&doc).unwrap();
    let wasm = wat::parse_str(
        r#"(module (memory (export "memory") 1)
             (func (export "run") (result i32)
               (loop $l (drop (memory.grow (i32.const 16))) (br $l)) (i32.const 0)))"#,
    )
    .unwrap();
    install(
        &ws,
        &build(&ws, "hog", EDIT, json!({"memory_mb": 2}), &wasm),
    );
    let stderr = fails(&ws, &["extension", "run", "hog", doc.to_str().unwrap()]);
    assert!(stderr.contains("sandbox-trap"), "{stderr}");
    assert_eq!(fs::read(&doc).unwrap(), before);

    // A module that declares more memory than its limit never starts.
    let big = wat::parse_str(
        r#"(module (memory (export "memory") 200) (func (export "run") (result i32) (i32.const 0)))"#,
    )
    .unwrap();
    install(
        &ws,
        &build(&ws, "bigmem", EDIT, json!({"memory_mb": 2}), &big),
    );
    let stderr = fails(&ws, &["extension", "run", "bigmem", doc.to_str().unwrap()]);
    assert!(stderr.contains("sandbox-trap"), "{stderr}");
}

#[test]
fn infinite_recursion_traps_instead_of_crashing_the_host() {
    let ws = Workspace::new("stack");
    let doc = new_doc(&ws);
    let wasm = wat::parse_str(
        r#"(module (memory (export "memory") 1)
             (func $f (result i32) (call $f))
             (func (export "run") (result i32) (call $f)))"#,
    )
    .unwrap();
    install(&ws, &build(&ws, "deep", EDIT, json!({}), &wasm));
    let stderr = fails(&ws, &["extension", "run", "deep", doc.to_str().unwrap()]);
    assert!(stderr.contains("sandbox-trap"), "{stderr}");
}

#[test]
fn escape_attempts_are_refused_before_anything_runs() {
    let ws = Workspace::new("escape");
    let dir = |name: &str, wat_src: &str| {
        let d = ws.path(name);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("extension.json"),
            manifest(name, EDIT, json!({})).to_string(),
        )
        .unwrap();
        fs::write(d.join("main.wasm"), wat::parse_str(wat_src).unwrap()).unwrap();
        d
    };
    let pack = |d: &Path| {
        fails(
            &ws,
            &[
                "extension",
                "pack",
                d.to_str().unwrap(),
                ws.path("x.penext").to_str().unwrap(),
            ],
        )
    };
    let wasi = dir(
        "wasi",
        r#"(module
          (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1) (func (export "run") (result i32) (i32.const 0)))"#,
    );
    assert!(pack(&wasi).contains("unsupported-capability"));
    let clock = dir(
        "clock",
        r#"(module (import "env" "now" (func (result i64)))
          (memory (export "memory") 1) (func (export "run") (result i32) (i32.const 0)))"#,
    );
    assert!(pack(&clock).contains("pentool.call"));
    let other = dir(
        "other",
        r#"(module (import "pentool" "spawn" (func (param i32) (result i32)))
          (memory (export "memory") 1) (func (export "run") (result i32) (i32.const 0)))"#,
    );
    assert!(pack(&other).contains("unsupported-capability"));
    assert!(
        !ws.path("x.penext").exists(),
        "a refused pack writes nothing"
    );
}

#[test]
fn out_of_bounds_pointers_and_call_floods_trap_cleanly() {
    let ws = Workspace::new("abuse");
    let doc = new_doc(&ws);
    let before = fs::read(&doc).unwrap();
    let oob = wat::parse_str(
        r#"(module
          (import "pentool" "call" (func $call (param i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "run") (result i32)
            (drop (call $call (i32.const 70000) (i32.const 100) (i32.const 0) (i32.const 10)))
            (i32.const 0)))"#,
    )
    .unwrap();
    install(&ws, &build(&ws, "oob", EDIT, json!({}), &oob));
    assert!(
        fails(&ws, &["extension", "run", "oob", doc.to_str().unwrap()])
            .contains("outside linear memory")
    );

    let flood = wat::parse_str(
        r#"(module
          (import "pentool" "call" (func $call (param i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 2)
          (data (i32.const 0) "{\"method\":\"api.info\"}")
          (func (export "run") (result i32)
            (loop $l
              (drop (call $call (i32.const 0) (i32.const 21) (i32.const 1024) (i32.const 4096)))
              (br $l))
            (i32.const 0)))"#,
    )
    .unwrap();
    install(
        &ws,
        &build(&ws, "flood", EDIT, json!({"cpu_ms": 60000}), &flood),
    );
    let stderr = fails(&ws, &["extension", "run", "flood", doc.to_str().unwrap()]);
    assert!(
        stderr.contains("call limit") || stderr.contains("cpu budget"),
        "{stderr}"
    );
    assert_eq!(fs::read(&doc).unwrap(), before);
}

#[test]
fn malformed_manifests_and_modules_are_rejected() {
    let ws = Workspace::new("manifest");
    let pack = |m: Value, wasm: &[u8]| {
        let d = ws.path("bad");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("extension.json"), m.to_string()).unwrap();
        fs::write(d.join("main.wasm"), wasm).unwrap();
        fails(
            &ws,
            &[
                "extension",
                "pack",
                d.to_str().unwrap(),
                ws.path("bad.penext").to_str().unwrap(),
            ],
        )
    };
    let good = guest(&[], 0);
    let mut m = manifest("bad", EDIT, json!({}));
    m["api"] = json!(2);
    assert!(pack(m, &good).contains("api 2"));
    let mut m = manifest("bad", EDIT, json!({}));
    m["capabilities"] = json!(["network.fetch"]);
    assert!(pack(m, &good).contains("unknown capability"));
    let mut m = manifest("bad", EDIT, json!({}));
    m["entry"] = json!("../escape.wasm");
    assert!(pack(m, &good).contains("unsafe-path"));
    let mut m = manifest("bad", EDIT, json!({}));
    m["limits"] = json!({"cpu_ms": 9_999_999});
    assert!(pack(m, &good).contains("limit-exceeded"));
    let mut m = manifest("bad", &["document.edit"], json!({}));
    m["capabilities"] = json!(["document.edit"]);
    assert!(pack(m, &good).contains("requires `document.read`"));
    let mut m = manifest("Bad Name", EDIT, json!({}));
    m["name"] = json!("Bad Name");
    assert!(pack(m, &good).contains("extension name"));
    assert!(pack(manifest("bad", EDIT, json!({})), b"not wasm").contains("malformed-resource"));
}

#[test]
fn tampered_packages_and_installs_fail_offline_verification() {
    let ws = Workspace::new("tamper");
    let doc = new_doc(&ws);
    let wasm = guest(&[apply_request(vec![add_box_op("t")])], 0);
    let package = build(&ws, "sealed", EDIT, json!({}), &wasm);

    // Swap the module inside the archive while keeping the old digest index.
    let original = fs::read(&package).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original)).unwrap();
    let forged_path = ws.path("forged.penext");
    {
        use std::io::{Read, Write};
        let mut writer = zip::ZipWriter::new(fs::File::create(&forged_path).unwrap());
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if entry.name() == "main.wasm" {
                bytes = guest(&[], 0);
            }
            writer
                .start_file(entry.name(), zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    assert!(
        fails(&ws, &["extension", "verify", forged_path.to_str().unwrap()])
            .contains("hash-mismatch")
    );

    // Editing an installed module after approval is detected at run time.
    install(&ws, &package);
    let installed = ws.path(".pentool/extensions/sealed/main.wasm");
    fs::write(&installed, guest(&[], 0)).unwrap();
    let stderr = fails(&ws, &["extension", "run", "sealed", doc.to_str().unwrap()]);
    assert!(stderr.contains("hash-mismatch"), "{stderr}");
    assert_eq!(
        ok(&ws, &["extension", "list"])["extensions"][0]["status"],
        "broken"
    );
}

#[test]
fn identical_runs_produce_identical_documents() {
    let ws = Workspace::new("determinism");
    let wasm = guest(&[apply_request(vec![add_box_op("same")])], 0);
    install(&ws, &build(&ws, "same", EDIT, json!({}), &wasm));
    let mut outputs = Vec::new();
    for n in 0..2 {
        let doc = ws.path(&format!("d{n}.pen"));
        quiet(
            &ws,
            &[
                "new",
                doc.to_str().unwrap(),
                "--width",
                "400",
                "--height",
                "300",
            ],
        );
        ok(&ws, &["extension", "run", "same", doc.to_str().unwrap()]);
        outputs.push(fs::read(&doc).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
}

fn example_package(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/extensions")
        .join(name)
        .join("package")
}

#[test]
fn shipped_examples_pack_install_and_run() {
    let ws = Workspace::new("examples");
    let doc = new_doc(&ws);
    for name in ["inventory", "stamp"] {
        let out = ws.path(&format!("{name}.penext"));
        quiet(
            &ws,
            &[
                "extension",
                "pack",
                example_package(name).to_str().unwrap(),
                out.to_str().unwrap(),
            ],
        );
        let verified = ok(&ws, &["extension", "verify", out.to_str().unwrap()]);
        assert_eq!(verified["name"], name);
        install(&ws, &out);
    }

    let inv = ok(
        &ws,
        &["extension", "run", "inventory", doc.to_str().unwrap()],
    );
    assert_eq!(inv["changed"], false);

    let args = json!({"text": "NEW", "x": 20, "y": 20}).to_string();
    let before = fs::read(&doc).unwrap();
    let dry = ok(
        &ws,
        &[
            "extension",
            "run",
            "stamp",
            doc.to_str().unwrap(),
            "--args",
            &args,
            "--dry-run",
        ],
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), before);

    let run = ok(
        &ws,
        &[
            "extension",
            "run",
            "stamp",
            doc.to_str().unwrap(),
            "--args",
            &args,
        ],
    );
    assert_eq!(run["changed"], true);
    assert!(ok(&ws, &["search", doc.to_str().unwrap(), "NEW"])
        .to_string()
        .contains("NEW"));
    quiet(&ws, &["undo", doc.to_str().unwrap()]);
    assert_eq!(fs::read(&doc).unwrap(), before);
}

struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn studio_server_lists_and_runs_extensions_with_a_revision_guard() {
    let ws = Workspace::new("serve");
    let doc = new_doc(&ws);
    let package = ws.path("stamp.penext");
    quiet(
        &ws,
        &[
            "extension",
            "pack",
            example_package("stamp").to_str().unwrap(),
            package.to_str().unwrap(),
        ],
    );
    install(&ws, &package);

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let _server = Server(
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["serve", doc.to_str().unwrap(), "--port", &port.to_string()])
            .current_dir(&ws.0)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::blocking::Client::new();
    assert!(
        (0..100).any(|_| {
            let up = client.get(format!("{base}/api/health")).send().is_ok();
            if !up {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            up
        }),
        "server did not start"
    );

    let listed: Value = client
        .get(format!("{base}/api/extensions"))
        .send()
        .unwrap()
        .text()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap();
    assert!(listed.to_string().contains("stamp"), "{listed}");

    let run = |body: Value| {
        let response = client
            .post(format!("{base}/api/extensions/run"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            serde_json::from_str::<Value>(&response.text().unwrap()).unwrap(),
        )
    };
    let current: Value = serde_json::from_str(
        &client
            .get(format!("{base}/api/document"))
            .send()
            .unwrap()
            .text()
            .unwrap(),
    )
    .unwrap();
    let rev = current["revision"].as_str().unwrap().to_string();
    let before = fs::read(&doc).unwrap();
    let (status, unguarded) = run(json!({"name":"stamp"}));
    assert_eq!(status, 400, "{unguarded}");
    let (status, stale) = run(json!({"name":"stamp","revision":"0000"}));
    assert_eq!(status, 400, "{stale}");
    assert_eq!(fs::read(&doc).unwrap(), before);

    let (status, dry) = run(json!({"name":"stamp","dry_run":true,"revision":rev}));
    assert_eq!(status, 200, "{dry}");
    assert_eq!(fs::read(&doc).unwrap(), before);

    let (status, done) = run(json!({"name":"stamp","args":{"text":"WEB"},"revision":rev}));
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["changed"], true);
    assert_ne!(fs::read(&doc).unwrap(), before);
    assert_eq!(run(json!({"name":"missing","revision":rev})).0, 400);
}

#[test]
fn batch_put_operations_respect_locked_layers() {
    let ws = Workspace::new("batchlock");
    let doc = new_doc(&ws);
    let path = doc.to_str().unwrap();
    quiet(&ws, &["layer", path, "set", "layer-1", "--locked", "true"]);
    let before = fs::read(&doc).unwrap();
    let ops = [
        add_box_op("locked-box"),
        json!({"type":"put-text","id":"locked-text","layer":"layer-1","content":"x","x":1,"y":1}),
        json!({"type":"put-path","id":"locked-path","layer":"layer-1","d":"M 0 0 L 5 5","stroke":"#000"}),
    ];
    for (i, op) in ops.iter().enumerate() {
        let file = ws.path(&format!("ops{i}.json"));
        fs::write(&file, json!([op]).to_string()).unwrap();
        let error = fails(&ws, &["batch", path, file.to_str().unwrap()]);
        assert!(error.contains("locked"), "{error}");
        assert_eq!(fs::read(&doc).unwrap(), before);
    }
}
