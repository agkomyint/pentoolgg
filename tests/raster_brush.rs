//! v0.10.0 item 3: textured stamp tips, smoothing and buildup through the CLI.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pentool-brush-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn document(&self, name: &str) -> String {
        let path = self.path(name);
        fs::copy("docs/fixtures/v4-scene.pen", &path).unwrap();
        ok(&[
            "raster", &path, "add", "p", "--width", "300", "--height", "200",
        ]);
        path
    }
    /// A 64x32 tip image: a black ring on white.
    fn tip_image(&self) -> String {
        let path = self.path("tip.png");
        let image = image::RgbaImage::from_fn(64, 32, |x, y| {
            let (dx, dy) = (x as f64 - 31.5, (y as f64 - 15.5) * 2.0);
            let d = (dx * dx + dy * dy).sqrt();
            if (18.0..28.0).contains(&d) {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        image.save(&path).unwrap();
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

fn load(path: &str) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn layer(doc: &Value) -> Value {
    doc["pages"][0]["layers"][0]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "p")
        .unwrap()
        .clone()
}

const TEXTURED: &str =
    r#"{"kind":"textured","tip":"ring","size":60,"spacing":0.3,"smoothing":0.5,"buildup":false}"#;

fn stroke(doc: &str, brush: &str) -> Output {
    run(&[
        "raster",
        doc,
        "stroke",
        "p",
        "--samples",
        "[[40,60],[120,140,0.8],[260,90]]",
        "--brush",
        brush,
        "--color",
        "#336699",
        "--seed",
        "3",
    ])
}

#[test]
fn tip_add_paint_replay_remove_and_release() {
    let ws = Workspace::new("tip");
    let doc = ws.document("doc.pen");
    let tip = ws.tip_image();
    let added = ok(&["raster", &doc, "tip-add", "ring", "--image", &tip]);
    let digest = added["result"]["tip"].as_str().unwrap().to_owned();
    assert!(digest.starts_with("sha256:"));
    assert_eq!(added["result"]["source_size"], json!([64, 32]));
    assert!(load(&doc)["raster_tiles"].get(&digest).is_some());

    // A duplicate name fails without touching the file.
    let before = fs::read(&doc).unwrap();
    assert!(fail(&["raster", &doc, "tip-add", "ring", "--image", &tip]).contains("conflict"));
    assert_eq!(fs::read(&doc).unwrap(), before);
    let listed = ok(&["raster", &doc, "tips"]);
    assert_eq!(listed["tips"], json!([{"name":"ring","tip":digest}]));

    // An unknown tip name fails without mutation.
    let out = stroke(&doc, r#"{"kind":"textured","tip":"nope","size":40}"#);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("known tips: [ring]"));
    assert_eq!(fs::read(&doc).unwrap(), before);

    let out = stroke(&doc, TEXTURED);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let painted = layer(&load(&doc));
    let brush = &painted["journal"][0]["brush"];
    assert_eq!(brush["tip"], json!(digest), "the journal pins the digest");
    assert_eq!(brush["smoothing"], json!(0.5));
    assert_eq!(brush["buildup"], json!(false));
    assert!(!painted["tiles"].as_object().unwrap().is_empty());
    let report = ok(&["raster", &doc, "verify", "--replay"]);
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["tips"]["checked"], 1);

    // Unnaming keeps the tile while the journal still records it, so replay works.
    let removed = ok(&["raster", &doc, "tip-remove", "ring"]);
    assert_eq!(removed["result"]["tiles_released"], 0);
    assert!(load(&doc)["raster_tiles"].get(&digest).is_some());
    assert_eq!(ok(&["raster", &doc, "verify", "--replay"])["ok"], true);

    // Compaction drops the journal; nothing pins the tip any more.
    ok(&["raster", &doc, "checkpoint", "p", "--compact"]);
    assert!(load(&doc)["raster_tiles"].get(&digest).is_none());
    assert_eq!(ok(&["raster", &doc, "verify", "--replay"])["ok"], true);

    // Undo restores the compacted journal and its tip tile.
    ok(&["undo", &doc]);
    assert!(load(&doc)["raster_tiles"].get(&digest).is_some());
}

#[test]
fn tips_and_textured_strokes_are_deterministic_across_documents() {
    let ws = Workspace::new("determinism");
    let tip = ws.tip_image();
    let mut hashes = Vec::new();
    for name in ["a.pen", "b.pen"] {
        let doc = ws.document(name);
        let added = ok(&["raster", &doc, "tip-add", "ring", "--image", &tip]);
        let out = stroke(&doc, TEXTURED);
        assert!(out.status.success());
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        hashes.push((
            added["result"]["tip"].clone(),
            result["result"]["tile_map_sha256"].clone(),
        ));
    }
    assert_eq!(hashes[0], hashes[1]);
    // Pinned on every release target: tip import, resampling, smoothing and the
    // textured stamp use only IEEE basic operations.
    assert_eq!(
        hashes[0].1,
        json!("sha256:97d2862846e27b55da3e86db0aea79d55ba2f73867fea5fa6ca79d1d3e8009e9")
    );
    // The same tip addressed by digest paints the same pixels as by name.
    let doc = ws.document("c.pen");
    ok(&["raster", &doc, "tip-add", "ring", "--image", &tip]);
    let by_digest = TEXTURED.replace("\"ring\"", &format!("{}", hashes[0].0));
    let out = stroke(&doc, &by_digest);
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["result"]["tile_map_sha256"], hashes[0].1);
}

#[test]
fn damaged_tips_are_reported_and_invalid_tip_input_is_refused() {
    let ws = Workspace::new("damage");
    let doc = ws.document("doc.pen");
    let tip = ws.tip_image();
    let digest = ok(&["raster", &doc, "tip-add", "ring", "--image", &tip])["result"]["tip"]
        .as_str()
        .unwrap()
        .to_owned();
    // Overwrite the tip's stored pixels with a different (valid) tile.
    let white = ws.path("white.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]))
        .save(&white)
        .unwrap();
    let other = ok(&["raster", &doc, "tip-add", "dot", "--image", &white])["result"]["tip"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut raw = load(&doc);
    raw["raster_tiles"][&digest]["data"] = raw["raster_tiles"][&other]["data"].clone();
    fs::write(&doc, serde_json::to_vec(&raw).unwrap()).unwrap();
    let out = run(&["raster", &doc, "verify"]);
    assert!(!out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["tips"]["damaged"],
        json!([{"name":"ring","tip":digest,"problem":"corrupt"}])
    );

    let fresh = ws.document("fresh.pen");
    let before = fs::read(&fresh).unwrap();
    let blank = ws.path("blank.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 255, 255]))
        .save(&blank)
        .unwrap();
    assert!(
        fail(&["raster", &fresh, "tip-add", "blank", "--image", &blank]).contains("no coverage")
    );
    assert!(fail(&["raster", &fresh, "tip-add", "bad name", "--image", &tip]).contains("tip name"));
    assert!(
        fail(&["raster", &fresh, "tip-add", "x", "--image", &tip, "--source", "luma"])
            .contains("darkness or alpha")
    );
    // White on transparent is a valid alpha tip.
    let alpha = ws.path("alpha.png");
    image::RgbaImage::from_fn(16, 16, |x, _| image::Rgba([255, 255, 255, (x * 16) as u8]))
        .save(&alpha)
        .unwrap();
    assert_eq!(fs::read(&fresh).unwrap(), before);
    let added = ok(&[
        "raster", &fresh, "tip-add", "fade", "--image", &alpha, "--source", "alpha",
    ]);
    assert!(added["result"]["covered_pixels"].as_u64().unwrap() > 0);
    let dry = ok(&["raster", &fresh, "--dry-run", "tip-remove", "fade"]);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(ok(&["raster", &fresh, "tips"])["returned"], 1);
}
