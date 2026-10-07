//! v0.10.0 item 7: healing brush and spot healing, through the CLI.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-heal-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    /// A document with a 300x200 layer `p`: gray field, a dark blemish at (100,100).
    fn document(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().into_owned();
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        ok(&[
            "raster", &path, "add", "p", "--width", "300", "--height", "200",
        ]);
        paint(&path, "[[150,100]]", 280, "#787878");
        paint(&path, "[[100,100]]", 10, "#141414");
        path
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
}

fn hash(doc: &str) -> String {
    ok(&["raster", doc, "info", "p"])["tile_map_sha256"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn paint(doc: &str, at: &str, size: u32, color: &str) {
    ok(&[
        "raster",
        doc,
        "stroke",
        "p",
        "--samples",
        at,
        "--brush",
        &format!(r#"{{"size":{size},"hardness":1,"flow":1}}"#),
        "--color",
        color,
    ]);
}

fn load(doc: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(doc).unwrap()).unwrap()
}

fn journal(doc: &str) -> Vec<Value> {
    fn find(value: &Value) -> Option<&Value> {
        match value {
            Value::Object(map) => {
                if map.get("id").and_then(Value::as_str) == Some("p") && map.contains_key("journal")
                {
                    Some(value)
                } else {
                    map.values().find_map(find)
                }
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    let raw = load(doc);
    find(&raw).unwrap()["journal"].as_array().unwrap().clone()
}

#[test]
fn heal_stroke_removes_a_blemish_replays_and_records_the_algorithm() {
    let ws = Workspace::new("stroke");
    let doc = ws.document("stroke.pen");
    ok(&[
        "raster",
        &doc,
        "clone-source",
        "p",
        "--x",
        "200",
        "--y",
        "100",
    ]);
    let result = ok(&[
        "raster",
        &doc,
        "heal-stroke",
        "p",
        "--samples",
        "[[100,100]]",
        "--brush",
        r#"{"size":30,"hardness":1,"texture":0.5}"#,
    ]);
    assert!(result["result"]["tiles_changed"].as_u64().unwrap() > 0);
    let last = journal(&doc).pop().unwrap();
    assert_eq!(last["blend"], "heal");
    assert_eq!(last["heal"]["algorithm"], 1);
    assert_eq!(last["brush"]["texture"], 0.5);
    let verify = ok(&["raster", &doc, "verify", "p", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
    assert_eq!(verify["layers"][0]["replay"]["status"], "match");

    // Healing the same way on a second copy is byte-identical.
    let other = ws.document("other.pen");
    ok(&[
        "raster",
        &other,
        "clone-source",
        "p",
        "--x",
        "200",
        "--y",
        "100",
    ]);
    ok(&[
        "raster",
        &other,
        "heal-stroke",
        "p",
        "--samples",
        "[[100,100]]",
        "--brush",
        r#"{"size":30,"hardness":1,"texture":0.5}"#,
    ]);
    assert_eq!(hash(&doc), hash(&other));
}

#[test]
fn heal_spot_picks_a_source_and_is_deterministic() {
    let ws = Workspace::new("spot");
    let (a, b) = (ws.document("a.pen"), ws.document("b.pen"));
    let before = hash(&a);
    let run_spot = |doc: &str| {
        ok(&[
            "raster",
            doc,
            "heal-spot",
            "p",
            "--x",
            "100",
            "--y",
            "100",
            "--radius",
            "8",
        ])
    };
    let result = run_spot(&a);
    run_spot(&b);
    assert_ne!(hash(&a), before);
    assert_eq!(hash(&a), hash(&b));
    assert_eq!(result["result"]["heal"]["algorithm"], 1);
    assert!(result["result"]["heal"]["source_offset"].is_array());
    let verify = ok(&["raster", &a, "verify", "--replay"]);
    assert_eq!(verify["ok"], true, "{verify}");
}

#[test]
fn bad_heal_requests_leave_the_document_unchanged() {
    let ws = Workspace::new("errors");
    let doc = ws.document("errors.pen");
    let before = fs::read(&doc).unwrap();
    let cases: [Vec<&str>; 5] = [
        // No source stored yet.
        vec![
            "raster",
            &doc,
            "heal-stroke",
            "p",
            "--samples",
            "[[100,100]]",
        ],
        // texture only applies to heal.
        vec![
            "raster",
            &doc,
            "stroke",
            "p",
            "--samples",
            "[[100,100]]",
            "--blend",
            "blur",
            "--brush",
            r#"{"texture":0.5}"#,
        ],
        // heal through the plain stroke command has no source.
        vec![
            "raster",
            &doc,
            "stroke",
            "p",
            "--samples",
            "[[100,100]]",
            "--blend",
            "heal",
        ],
        vec![
            "raster",
            &doc,
            "heal-spot",
            "p",
            "--x",
            "100",
            "--y",
            "100",
            "--radius",
            "0",
        ],
        vec![
            "raster",
            &doc,
            "heal-spot",
            "p",
            "--x",
            "9999",
            "--y",
            "100",
        ],
    ];
    for args in cases {
        fail(&args);
    }
    assert_eq!(fs::read(&doc).unwrap(), before);
    let dry = ok(&[
        "raster",
        &doc,
        "--dry-run",
        "heal-spot",
        "p",
        "--x",
        "100",
        "--y",
        "100",
    ]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read(&doc).unwrap(), before);
}
