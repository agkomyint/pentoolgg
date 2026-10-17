//! v0.14.0 tasks 13-14: IDML import and export.
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pentool-idml-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
        "{args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn fails(args: &[&str]) -> String {
    let out = run(args);
    assert!(!out.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn collect(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, base, out);
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&path).unwrap()));
        }
    }
}

fn zip_entries(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            let options = if name == "mimetype" { stored } else { deflated };
            zip.start_file(name.as_str(), options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

fn fixture_entries() -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/idml/minimal");
    let mut entries = Vec::new();
    collect(&root, &root, &mut entries);
    entries.sort_by(|a, b| {
        // mimetype first, then sorted names.
        (a.0 != "mimetype", &a.0).cmp(&(b.0 != "mimetype", &b.0))
    });
    entries
}

fn fixture_idml(ws: &Workspace) -> String {
    let path = ws.path("minimal.idml");
    std::fs::write(&path, zip_entries(&fixture_entries())).unwrap();
    path
}

fn import(ws: &Workspace, idml: &str, name: &str) -> (Value, Value) {
    let out = ws.path(name);
    let report = ws.path(&format!("{name}.report.json"));
    let summary = ok(&["idml-import", idml, &out, "--report", &report]);
    let doc: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let full: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(summary["counts"], full["counts"]);
    (doc, full)
}

fn items<'a>(report: &'a Value, status: &str, element: &str) -> Vec<&'a Value> {
    report["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["status"] == status && i["element"] == element)
        .collect()
}

fn frames(doc: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for page in doc["pages"].as_array().unwrap() {
        for layer in page["layers"].as_array().unwrap() {
            for node in layer["nodes"].as_array().unwrap() {
                if node["kind"] == "text-frame" {
                    out.push(node);
                }
            }
        }
    }
    out
}

#[test]
fn imports_spreads_frames_stories_and_styles() {
    let ws = Workspace::new("basic");
    let idml = fixture_idml(&ws);
    let (doc, report) = import(&ws, &idml, "out.pen");

    assert_eq!(doc["version"], 9);
    assert_eq!(doc["pages"].as_array().unwrap().len(), 2);
    assert_eq!(doc["pages"][0]["canvas"]["width"], 612.0);
    assert_eq!(doc["pages"][0]["canvas"]["height"], 792.0);
    assert_eq!(
        doc["layout"]["spreads"][0]["pages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(doc["layout"]["spreads"][0]["facing"], true);
    assert_eq!(
        doc["layout"]["parents"][0]["margins"],
        serde_json::json!([54.0, 54.0, 54.0, 54.0])
    );
    assert_eq!(
        doc["pages"][0]["layout"]["parent"],
        doc["layout"]["parents"][0]["id"]
    );
    assert_eq!(doc["layout"]["sections"][0]["style"], "arabic");

    // Threaded frames keep their geometry relative to their own page.
    let found = frames(&doc);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0]["id"], "u100");
    assert_eq!(found[0]["next"], "u101");
    assert_eq!(found[1]["next"], Value::Null);
    assert_eq!(found[0]["x"], 54.0);
    assert_eq!(found[0]["y"], 72.0);
    assert_eq!(found[1]["x"], 54.0);
    assert_eq!(found[0]["inset"], serde_json::json!([2.0, 5.0, 4.0, 3.0]));

    // Story: two paragraphs, a line-break special and a page-number special.
    let story = &doc["layout"]["stories"][0];
    assert_eq!(story["id"], "u10");
    let paragraphs = story["paragraphs"].as_array().unwrap();
    assert_eq!(paragraphs.len(), 2);
    assert_eq!(paragraphs[0]["style"], "Heading");
    assert_eq!(
        paragraphs[0]["runs"][0]["text"],
        "Rivers carry more than water"
    );
    assert_eq!(paragraphs[1]["style"], "Body");
    let runs = paragraphs[1]["runs"].as_array().unwrap();
    assert!(runs.iter().any(|r| r["special"] == "line-break"));
    assert!(runs.iter().any(|r| r["special"] == "page-number"));
    assert!(runs
        .iter()
        .any(|r| r["text"] == "silt" && r["style"] == "Emphasis"));

    // Styles.
    let body = &doc["layout"]["styles"]["paragraph"]["Body"];
    assert_eq!(body["size"], 10.0);
    assert_eq!(body["align"], "justify");
    assert_eq!(body["hyphenate"], true);
    assert_eq!(body["space_after"], 6.0);
    let heading = &doc["layout"]["styles"]["paragraph"]["Heading"];
    assert_eq!(heading["weight"], 700);
    assert_eq!(heading["based_on"], "Body");
    assert_eq!(heading["next"], "Body");
    assert_eq!(
        doc["extension"]["idml"]["style_properties"]["paragraph:Heading"]["FillColor"],
        "Color/Brand"
    );
    assert_eq!(
        doc["layout"]["styles"]["character"]["Emphasis"]["italic"],
        true
    );

    // Page items: ellipse, line and a group on the layer.
    let nodes = doc["pages"][1]["layers"][0]["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["kind"] == "ellipse"));
    assert!(nodes.iter().any(|n| n["kind"] == "line"));
    let ellipse = nodes.iter().find(|n| n["kind"] == "ellipse").unwrap();
    assert_eq!(ellipse["radius_x"], 40.0);
    assert_eq!(ellipse["radius_y"], 30.0);
    let on_first = doc["pages"][0]["layers"][0]["nodes"].as_array().unwrap();
    let group = on_first.iter().find(|n| n["kind"] == "group").unwrap();
    assert_eq!(group["children"][0]["kind"], "rect");

    // Report: nothing silent.
    assert_eq!(items(&report, "unsupported", "Button").len(), 1);
    assert_eq!(items(&report, "approximated", "Font").len(), 1);
    assert_eq!(items(&report, "approximated", "Color").len(), 1); // CMYK black
    assert_eq!(items(&report, "carried", "TextFrame").len(), 1); // master folio
    let expected: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/idml/minimal.expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(report["counts"], expected["counts"]);
    assert_eq!(report["pages"], expected["pages"]);
    assert_eq!(report["stories"], expected["stories"]);
    assert_eq!(report["missing_fonts"][0], "Atkinson Hyperlegible Bold");
    // KeepLinesTogether has no import mapping yet: carried, not dropped.
    assert_eq!(
        doc["extension"]["idml"]["style_properties"]["paragraph:Body"]["KeepLinesTogether"],
        "true"
    );
}

#[test]
fn import_is_deterministic_and_never_overwrites_silently() {
    let ws = Workspace::new("determinism");
    let idml = fixture_idml(&ws);
    import(&ws, &idml, "a.pen");
    import(&ws, &idml, "b.pen");
    assert_eq!(
        std::fs::read(ws.path("a.pen")).unwrap(),
        std::fs::read(ws.path("b.pen")).unwrap()
    );
    assert_eq!(
        std::fs::read(ws.path("a.pen.report.json")).unwrap(),
        std::fs::read(ws.path("b.pen.report.json")).unwrap()
    );
    let before = std::fs::read(ws.path("a.pen")).unwrap();
    let error = fails(&["idml-import", &idml, &ws.path("a.pen")]);
    assert!(error.contains("[policy-denied]"), "{error}");
    assert_eq!(before, std::fs::read(ws.path("a.pen")).unwrap());
}

#[test]
fn hostile_packages_are_refused_without_output() {
    let ws = Workspace::new("hostile");
    let write = |name: &str, entries: Vec<(String, Vec<u8>)>| {
        let path = ws.path(name);
        std::fs::write(&path, zip_entries(&entries)).unwrap();
        path
    };
    let base = fixture_entries();

    let mut traversal = base.clone();
    traversal.push(("../evil.xml".into(), b"<a/>".to_vec()));
    let path = write("traversal.idml", traversal);
    let out = ws.path("t.pen");
    let error = fails(&["idml-import", &path, &out]);
    assert!(error.contains("[unsafe-path]"), "{error}");
    assert!(!Path::new(&out).exists());

    let mut absolute = base.clone();
    absolute.push(("/etc/passwd".into(), b"x".to_vec()));
    let error = fails(&[
        "idml-import",
        &write("abs.idml", absolute),
        &ws.path("a.pen"),
    ]);
    assert!(error.contains("[unsafe-path]"), "{error}");

    let mut dtd = base.clone();
    for (name, data) in &mut dtd {
        if name == "designmap.xml" {
            *data = b"<?xml version=\"1.0\"?><!DOCTYPE d [<!ENTITY x \"boom\">]><Document>&x;</Document>".to_vec();
        }
    }
    let error = fails(&["idml-import", &write("dtd.idml", dtd), &ws.path("d.pen")]);
    assert!(error.contains("[unsafe-input]"), "{error}");

    let mut bomb = base.clone();
    bomb.push(("Resources/Padding.xml".into(), vec![b' '; 4 * 1024 * 1024]));
    let error = fails(&["idml-import", &write("bomb.idml", bomb), &ws.path("b.pen")]);
    assert!(error.contains("[limit-exceeded]"), "{error}");

    let mut deep = base.clone();
    let nested = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
    for (name, data) in &mut deep {
        if name == "Stories/Story_u10.xml" {
            *data = nested.clone().into_bytes();
        }
    }
    let error = fails(&["idml-import", &write("deep.idml", deep), &ws.path("e.pen")]);
    assert!(error.contains("[limit-exceeded]"), "{error}");

    let wrong_mime: Vec<(String, Vec<u8>)> = base
        .iter()
        .filter(|(n, _)| n != "mimetype")
        .cloned()
        .collect();
    let error = fails(&[
        "idml-import",
        &write("nomime.idml", wrong_mime),
        &ws.path("m.pen"),
    ]);
    assert!(error.contains("mimetype"), "{error}");

    let indd = ws.path("native.indd");
    std::fs::write(
        &indd,
        [
            0x06u8, 0x06, 0xED, 0xF5, 0xD8, 0x1D, 0x46, 0xE5, 0x9D, 0x0E, 0x00, 0x00,
        ],
    )
    .unwrap();
    let error = fails(&["idml-import", &indd, &ws.path("n.pen")]);
    assert!(error.contains("native InDesign files"), "{error}");

    let notzip = ws.path("junk.idml");
    std::fs::write(&notzip, b"not a zip").unwrap();
    let error = fails(&["idml-import", &notzip, &ws.path("j.pen")]);
    assert!(error.contains("[malformed-resource]"), "{error}");
    assert!(!Path::new(&ws.path("j.pen")).exists());
}

#[test]
fn missing_story_part_is_reported_not_fatal() {
    let ws = Workspace::new("missing-story");
    let entries: Vec<(String, Vec<u8>)> = fixture_entries()
        .into_iter()
        .filter(|(n, _)| n != "Stories/Story_u10.xml")
        .collect();
    let path = ws.path("nostory.idml");
    std::fs::write(&path, zip_entries(&entries)).unwrap();
    let (doc, report) = import(&ws, &path, "o.pen");
    assert_eq!(doc["layout"]["stories"][0]["id"], "u10");
    assert_eq!(items(&report, "unsupported", "src").len(), 1);
    assert_eq!(items(&report, "approximated", "TextFrame").len(), 1);
}

fn export(ws: &Workspace, pen: &str, name: &str, extra: &[&str]) -> (String, Value) {
    let out = ws.path(name);
    let report = ws.path(&format!("{name}.report.json"));
    let mut args = vec![
        "idml-export",
        pen,
        out.as_str(),
        "--report",
        report.as_str(),
    ];
    args.extend_from_slice(extra);
    ok(&args);
    let full: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    (out, full)
}

#[test]
fn round_trip_preserves_supported_features_and_carried_blocks() {
    let ws = Workspace::new("roundtrip");
    let idml = fixture_idml(&ws);
    let (first, _) = import(&ws, &idml, "first.pen");
    let (exported, report) = export(&ws, &ws.path("first.pen"), "out.idml", &[]);
    assert_eq!(report["counts"]["unsupported"], 0);
    assert_eq!(report["counts"]["approximated"], 0);
    // The master text frame returns; nothing is dropped without being listed.
    assert!(!items(&report, "carried", "TextFrame").is_empty());
    let (second, _) = import(&ws, &exported, "second.pen");

    for key in ["pages", "layout"] {
        assert_eq!(first[key], second[key], "{key} changed in the round trip");
    }
    // Carried blocks (Button, master folio, style properties) survive.
    let blocks = |doc: &Value| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = doc["extension"]["idml"]["carried"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["element"].as_str().unwrap().to_string(),
                    c["xml"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(blocks(&first), blocks(&second));
    assert_eq!(
        first["extension"]["idml"]["style_properties"],
        second["extension"]["idml"]["style_properties"]
    );
    assert_eq!(
        first["extension"]["idml"]["swatches"],
        second["extension"]["idml"]["swatches"]
    );
}

#[test]
fn export_is_deterministic_and_has_a_stored_mimetype_first() {
    let ws = Workspace::new("export-determinism");
    let idml = fixture_idml(&ws);
    import(&ws, &idml, "doc.pen");
    let mislabeled = ws.path("output.zip");
    let error = fails(&["idml-export", &ws.path("doc.pen"), &mislabeled]);
    assert!(error.contains("[invalid-input]"), "{error}");
    assert!(!Path::new(&mislabeled).exists());
    let (a, _) = export(&ws, &ws.path("doc.pen"), "a.idml", &[]);
    let (b, _) = export(&ws, &ws.path("doc.pen"), "b.idml", &[]);
    let bytes = std::fs::read(&a).unwrap();
    assert_eq!(bytes, std::fs::read(&b).unwrap());
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let first = zip.by_index(0).unwrap();
    assert_eq!(first.name(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    let error = fails(&["idml-export", &ws.path("doc.pen"), &a]);
    assert!(error.contains("[policy-denied]"), "{error}");
}

#[test]
fn features_without_an_idml_equivalent_fail_unless_approximation_is_allowed() {
    let ws = Workspace::new("approximation");
    let pen = ws.path("native.pen");
    ok(&["new", &pen, "--width", "400", "--height", "300"]);
    let doc: Value = serde_json::from_slice(&std::fs::read(&pen).unwrap()).unwrap();
    let mut doc = doc;
    doc["pages"][0]["layers"][0]["nodes"] = serde_json::json!([
        {"kind": "rect", "id": "r1", "x": 10, "y": 20, "width": 100, "height": 50,
         "style": {"fill": {"fallback": "#336699"}, "stroke": {"fallback": "none"}, "stroke_width": {"fallback": 0}},
         "opacity": 0.5}
    ]);
    std::fs::write(&pen, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    let out = ws.path("native.idml");
    let error = fails(&["idml-export", &pen, &out]);
    assert!(error.contains("--allow-approximation"), "{error}");
    assert!(!Path::new(&out).exists());
    let (path, report) = export(&ws, &pen, "ok.idml", &["--allow-approximation"]);
    assert_eq!(report["counts"]["approximated"], 1);
    assert!(Path::new(&path).exists());
    // The result is a package Pentool itself can open again.
    let (again, _) = import(&ws, &path, "again.pen");
    let rect = &again["pages"][0]["layers"][0]["nodes"][0];
    assert_eq!(rect["kind"], "rect");
    assert_eq!(rect["x"], 10.0);
    assert_eq!(rect["y"], 20.0);
    assert_eq!(rect["style"]["fill"]["fallback"], "#336699");
}

#[test]
fn export_rejects_non_idml_output_names() {
    let ws = Workspace::new("export-extension");
    let idml = fixture_idml(&ws);
    import(&ws, &idml, "doc.pen");
    // IDML bytes under a `.zip` (or any other) name would mislabel the package.
    for name in ["pkg.zip", "pkg.pen", "pkg"] {
        let out = ws.path(name);
        let error = fails(&["idml-export", &ws.path("doc.pen"), &out]);
        assert!(error.contains("must end in .idml"), "{name}: {error}");
        assert!(!Path::new(&out).exists(), "{name} must not be written");
    }
    // The check fires before any input read: a missing input with a bad output
    // extension still reports the extension error.
    let error = fails(&["idml-export", &ws.path("missing.pen"), &ws.path("pkg.zip")]);
    assert!(error.contains("must end in .idml"), "{error}");
}
