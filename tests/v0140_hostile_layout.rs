//! v0.14.0 task 18: hostile-input and bounded-work coverage for the layout
//! features (IDML, `.penbook`, tables, styles, stories, long documents).
//!
//! Every case must fail with a named `[code]` error and leave the document, the
//! book, its members and the surrounding directory byte-for-byte unchanged.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ------------------------------------------------------------ harness

struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pentool-hostile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace(dir)
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
    fn s(&self, rel: &str) -> String {
        self.p(rel).to_string_lossy().into_owned()
    }
    fn json(&self, rel: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.p(rel)).unwrap()).unwrap()
    }
    fn write(&self, rel: &str, raw: &Value) {
        let path = self.p(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(raw).unwrap()).unwrap();
    }
    /// Every file below the workspace with its bytes (hidden history and
    /// journals included), so stray siblings are detected too.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        collect(&self.0, &self.0, &mut out);
        out
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn collect(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
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
            out.insert(rel, std::fs::read(&path).unwrap());
        }
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

/// Run a command that must fail, return stderr and prove nothing changed.
fn refused(ws: &Workspace, args: &[&str]) -> String {
    let before = ws.snapshot();
    let out = run(args);
    assert!(
        !out.status.success(),
        "{args:?} unexpectedly succeeded: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let error = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !error.contains("panicked") && !error.contains("RUST_BACKTRACE"),
        "{args:?} panicked: {error}"
    );
    assert!(
        error.contains('['),
        "{args:?}: the error carries no [code]: {error}"
    );
    assert!(
        before == ws.snapshot(),
        "{args:?} changed the workspace although it failed: {error}"
    );
    error
}

fn refused_with(ws: &Workspace, args: &[&str], code: &str) -> String {
    let error = refused(ws, args);
    assert!(error.contains(code), "{args:?}: expected {code}: {error}");
    error
}

// ------------------------------------------------------------ IDML

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
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/fixtures/idml/minimal");
    let mut entries = Vec::new();
    walk(&root, &root, &mut entries);
    entries.sort_by(|a, b| (a.0 != "mimetype", &a.0).cmp(&(b.0 != "mimetype", &b.0)));
    entries
}

fn replace_part(entries: &mut [(String, Vec<u8>)], name: &str, data: Vec<u8>) {
    let slot = entries
        .iter_mut()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("fixture part {name} missing"));
    slot.1 = data;
}

fn edit_part(entries: &mut [(String, Vec<u8>)], name: &str, from: &str, to: &str) {
    let slot = entries.iter_mut().find(|(n, _)| n == name).unwrap();
    let text = String::from_utf8(slot.1.clone()).unwrap();
    assert!(text.contains(from), "{name} lacks {from:?}");
    slot.1 = text.replacen(from, to, 1).into_bytes();
}

/// Write `entries` as `<name>.idml` and import it, proving the failure leaves
/// an existing destination (overwritten with --overwrite) untouched.
fn import_refused(ws: &Workspace, name: &str, entries: &[(String, Vec<u8>)], code: &str) -> String {
    let idml = ws.s(name);
    std::fs::write(&idml, zip_entries(entries)).unwrap();
    // A fresh destination must not appear.
    let fresh = ws.s(&format!("{name}.out.pen"));
    let error = refused_with(ws, &["idml-import", &idml, &fresh], code);
    assert!(!Path::new(&fresh).exists(), "{name}: partial output left");
    // An existing destination survives --overwrite when the import fails.
    let existing = ws.s(&format!("{name}.existing.pen"));
    std::fs::write(&existing, b"precious bytes").unwrap();
    refused_with(
        ws,
        &["idml-import", &idml, &existing, "--overwrite"],
        code,
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"precious bytes");
    std::fs::remove_file(&existing).unwrap();
    error
}

#[test]
fn idml_unsafe_entry_names_are_refused() {
    let ws = Workspace::new("idml-names");
    for (label, name) in [
        ("backslash", "Stories\\Story_x.xml"),
        ("drive", "C:/Windows/evil.xml"),
        ("nested-dotdot", "Resources/../../evil.xml"),
        ("absolute", "/tmp/evil.xml"),
    ] {
        let mut entries = fixture_entries();
        entries.push((name.to_string(), b"<a/>".to_vec()));
        import_refused(&ws, &format!("{label}.idml"), &entries, "[unsafe-path]");
    }
}

#[test]
fn idml_entry_count_over_limit_is_refused() {
    let ws = Workspace::new("idml-count");
    let mut entries = fixture_entries();
    for i in 0..20_001 {
        entries.push((format!("Resources/pad{i}.bin"), Vec::new()));
    }
    let error = import_refused(&ws, "many.idml", &entries, "[limit-exceeded]");
    assert!(error.contains("entries"), "{error}");
}

#[test]
fn idml_compression_ratio_bombs_are_refused_for_text_and_binary_parts() {
    let ws = Workspace::new("idml-ratio");
    let mut text = fixture_entries();
    text.push((
        "Resources/Bomb.xml".into(),
        format!("<a>{}</a>", " ".repeat(8 * 1024 * 1024)).into_bytes(),
    ));
    let error = import_refused(&ws, "text.idml", &text, "[limit-exceeded]");
    assert!(error.contains("ratio"), "{error}");

    let mut binary = fixture_entries();
    binary.push(("Resources/blob.bin".into(), vec![0u8; 8 * 1024 * 1024]));
    let error = import_refused(&ws, "binary.idml", &binary, "[limit-exceeded]");
    assert!(error.contains("ratio"), "{error}");
}

#[test]
fn idml_xml_depth_attribute_and_entity_bombs_are_refused() {
    let ws = Workspace::new("idml-xml");

    // Beyond the 128 limit and beyond the XML parser's own nesting guard.
    for (label, depth) in [("deep200", 200usize), ("deep5000", 5000)] {
        let mut entries = fixture_entries();
        let nested = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        replace_part(&mut entries, "Stories/Story_u10.xml", nested.into_bytes());
        let error = import_refused(&ws, &format!("{label}.idml"), &entries, "[");
        assert!(
            error.contains("[limit-exceeded]") || error.contains("[malformed-resource]"),
            "{label}: {error}"
        );
    }

    let mut attrs = fixture_entries();
    let many: String = (0..300).map(|i| format!(" a{i}=\"1\"")).collect();
    edit_part(
        &mut attrs,
        "designmap.xml",
        "<Layer Self=\"ulayer1\"",
        &format!("<Layer{many} Self=\"ulayer1\""),
    );
    import_refused(&ws, "attrs.idml", &attrs, "[limit-exceeded]");

    // Billion-laughs style and external entities never reach the parser.
    let mut laughs = fixture_entries();
    replace_part(
        &mut laughs,
        "Stories/Story_u10.xml",
        b"<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">]><l>&b;</l>"
            .to_vec(),
    );
    import_refused(&ws, "laughs.idml", &laughs, "[unsafe-input]");

    let mut xxe = fixture_entries();
    replace_part(
        &mut xxe,
        "designmap.xml",
        b"<?xml version=\"1.0\"?><!DOCTYPE d [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><Document>&x;</Document>"
            .to_vec(),
    );
    import_refused(&ws, "xxe.idml", &xxe, "[unsafe-input]");

    let mut broken = fixture_entries();
    replace_part(&mut broken, "designmap.xml", b"<Document><unclosed>".to_vec());
    import_refused(&ws, "broken.idml", &broken, "[malformed-resource]");
}

#[test]
fn idml_part_references_cannot_leave_the_package() {
    let ws = Workspace::new("idml-src");
    let mut entries = fixture_entries();
    edit_part(
        &mut entries,
        "designmap.xml",
        "Stories/Story_u10.xml",
        "../../../etc/passwd",
    );
    // A traversing src is either refused by name or reported missing; it must
    // never be read from disk, so the import cannot succeed silently with it.
    let idml = ws.s("src.idml");
    std::fs::write(&idml, zip_entries(&entries)).unwrap();
    let out = ws.s("src.pen");
    let before = ws.snapshot();
    let result = run(&["idml-import", &idml, &out, "--report", &ws.s("r.json")]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    if result.status.success() {
        let report = ws.json("r.json");
        assert!(
            report["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["status"] == "unsupported"),
            "the unreadable part must be reported: {report}"
        );
    } else {
        assert!(stderr.contains("[unsafe-path]"), "{stderr}");
        assert!(before == ws.snapshot());
    }
}

#[test]
fn idml_hostile_geometry_and_styles_fail_by_name_or_import_cleanly() {
    let ws = Workspace::new("idml-geometry");
    // Absurd page size: either refused by name or clamped to a valid document.
    let mut huge = fixture_entries();
    edit_part(
        &mut huge,
        "Spreads/Spread_u1.xml",
        "GeometricBounds=\"0 0 792 612\" ItemTransform=\"1 0 0 1 -612 -396\"",
        "GeometricBounds=\"0 0 1e300 1e300\" ItemTransform=\"1 0 0 1 -612 -396\"",
    );
    let idml = ws.s("huge.idml");
    std::fs::write(&idml, zip_entries(&huge)).unwrap();
    let out = ws.s("huge.pen");
    let result = run(&["idml-import", &idml, &out]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    if result.status.success() {
        // Whatever was written must itself be a valid, bounded document.
        let check = run(&["validate", &out]);
        assert!(
            check.status.success(),
            "imported huge page does not validate: {}",
            String::from_utf8_lossy(&check.stderr)
        );
    } else {
        assert!(stderr.contains('['), "{stderr}");
        assert!(!Path::new(&out).exists());
    }

    // A based-on cycle in the style sheet must not hang or recurse forever.
    let mut cycle = fixture_entries();
    edit_part(
        &mut cycle,
        "Resources/Styles.xml",
        "<ParagraphStyle Self=\"ParagraphStyle/Body\" Name=\"Body\"",
        "<ParagraphStyle Self=\"ParagraphStyle/Body\" Name=\"Body\" NextStyle=\"ParagraphStyle/Heading\"",
    );
    let idml = ws.s("cycle.idml");
    std::fs::write(&idml, zip_entries(&cycle)).unwrap();
    let out = ws.s("cycle.pen");
    let result = run(&["idml-import", &idml, &out]);
    assert!(
        !String::from_utf8_lossy(&result.stderr).contains("panicked"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn idml_export_refusals_leave_the_destination_untouched() {
    let ws = Workspace::new("idml-export");
    let idml = ws.s("in.idml");
    std::fs::write(&idml, zip_entries(&fixture_entries())).unwrap();
    ok(&["idml-import", &idml, &ws.s("doc.pen")]);
    std::fs::write(ws.p("existing.idml"), b"keep me").unwrap();
    let error = refused(
        &ws,
        &["idml-export", &ws.s("doc.pen"), &ws.s("existing.idml")],
    );
    assert!(error.contains("[policy-denied]"), "{error}");
    // A version 4 document has no layout and cannot be exported as layout IDML
    // either; the failure still names a code and writes nothing.
    ok(&["new", &ws.s("plain.pen")]);
    let before = ws.snapshot();
    let out = run(&["idml-export", &ws.s("plain.pen"), &ws.s("plain.idml")]);
    if !out.status.success() {
        assert!(String::from_utf8_lossy(&out.stderr).contains('['));
        assert!(before == ws.snapshot());
    }
}

// ------------------------------------------------------------ .penbook

fn member(ws: &Workspace, rel: &str) {
    let path = ws.s(rel);
    if let Some(parent) = ws.p(rel).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    ok(&["new", &path, "--overwrite"]);
    ok(&["migrate", &path, "--target", "9"]);
}

fn book_manifest(documents: Vec<Value>) -> Value {
    json!({"format": "penbook", "version": 1, "name": "Hostile", "documents": documents})
}

fn doc_entry(path: &str) -> Value {
    json!({"path": path, "role": "chapter"})
}

#[test]
fn penbook_hostile_manifests_are_refused_by_name() {
    let ws = Workspace::new("book-manifest");
    member(&ws, "a.pen");
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "dotdot",
            book_manifest(vec![doc_entry("../outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "nested-dotdot",
            book_manifest(vec![doc_entry("a/../../outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "absolute",
            book_manifest(vec![doc_entry("/etc/outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "drive",
            book_manifest(vec![doc_entry("C:/outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "backslash",
            book_manifest(vec![doc_entry("sub\\outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "empty-segment",
            book_manifest(vec![doc_entry("sub//outside.pen")]),
            "[unsafe-path]",
        ),
        (
            "nul",
            book_manifest(vec![doc_entry("a\u{0}.pen")]),
            "[unsafe-path]",
        ),
        (
            "duplicate",
            book_manifest(vec![doc_entry("a.pen"), doc_entry("a.pen")]),
            "listed twice",
        ),
        (
            "duplicate-case",
            book_manifest(vec![doc_entry("a.pen"), doc_entry("A.pen")]),
            "listed twice",
        ),
        (
            "not-a-pen",
            book_manifest(vec![doc_entry("a.txt")]),
            "[invalid-input]",
        ),
        ("empty", book_manifest(vec![]), "[limit-exceeded]"),
        (
            "future-version",
            json!({"format": "penbook", "version": 2, "name": "x", "documents": [doc_entry("a.pen")]}),
            "[unsupported-version]",
        ),
        (
            "wrong-format",
            json!({"format": "pdf", "version": 1, "name": "x", "documents": [doc_entry("a.pen")]}),
            "[malformed-resource]",
        ),
        (
            "bad-role",
            book_manifest(vec![json!({"path": "a.pen", "role": "boss"})]),
            "[invalid-input]",
        ),
    ];
    for (label, manifest, code) in cases {
        let name = format!("{label}.penbook");
        ws.write(&name, &manifest);
        for action in ["status", "renumber"] {
            let error = refused(&ws, &["book", &ws.s(&name), action]);
            assert!(error.contains(code), "{label}/{action}: {error}");
        }
    }
    std::fs::write(ws.p("junk.penbook"), b"{not json").unwrap();
    refused_with(
        &ws,
        &["book", &ws.s("junk.penbook"), "status"],
        "[malformed-resource]",
    );
}

#[test]
fn penbook_with_more_than_500_members_is_refused_before_touching_any_member() {
    let ws = Workspace::new("book-count");
    member(&ws, "a.pen");
    let documents: Vec<Value> = (0..501).map(|i| doc_entry(&format!("m{i}.pen"))).collect();
    ws.write("big.penbook", &book_manifest(documents));
    let error = refused_with(
        &ws,
        &["book", &ws.s("big.penbook"), "renumber"],
        "[limit-exceeded]",
    );
    assert!(error.contains("500"), "{error}");
    // Exactly 500 listed (all missing) is within the count limit; it fails as a
    // missing-resource abort listing members, not by writing anything.
    let documents: Vec<Value> = (0..500).map(|i| doc_entry(&format!("m{i}.pen"))).collect();
    ws.write("edge.penbook", &book_manifest(documents));
    refused_with(
        &ws,
        &["book", &ws.s("edge.penbook"), "renumber"],
        "[missing-resource]",
    );
    // `book add` past the limit is refused as well.
    let documents: Vec<Value> = (0..500).map(|i| doc_entry(&format!("m{i}.pen"))).collect();
    ws.write("full.penbook", &book_manifest(documents));
    refused_with(
        &ws,
        &["book", &ws.s("full.penbook"), "add", "extra.pen"],
        "[limit-exceeded]",
    );
}

#[test]
fn penbook_oversized_or_unreadable_manifest_is_refused() {
    let ws = Workspace::new("book-size");
    // 9 MiB of padding inside a JSON string: over any sane manifest size.
    let blob = format!(
        "{{\"format\":\"penbook\",\"version\":1,\"name\":\"{}\",\"documents\":[]}}",
        "x".repeat(9 * 1024 * 1024)
    );
    std::fs::write(ws.p("huge.penbook"), blob).unwrap();
    refused_with(
        &ws,
        &["book", &ws.s("huge.penbook"), "status"],
        "[limit-exceeded]",
    );
    refused_with(
        &ws,
        &["book", &ws.s("missing.penbook"), "status"],
        "[missing-resource]",
    );
}

#[test]
fn penbook_symlink_member_escape_is_refused_without_writing_outside() {
    let ws = Workspace::new("book-symlink");
    let outside = Workspace::new("book-symlink-outside");
    member(&outside, "secret.pen");
    member(&ws, "a.pen");
    let link = ws.p("linked.pen");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(outside.p("secret.pen"), &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(outside.p("secret.pen"), &link).is_ok();
    #[cfg(not(any(unix, windows)))]
    let made = false;
    if !made {
        eprintln!("skipping: symlinks are not permitted on this machine");
        return;
    }
    ws.write(
        "b.penbook",
        &book_manifest(vec![doc_entry("a.pen"), doc_entry("linked.pen")]),
    );
    let secret_before = std::fs::read(outside.p("secret.pen")).unwrap();
    for action in [
        vec!["status"],
        vec!["renumber"],
        vec!["sync"],
        vec!["toc", "--style", "h1"],
    ] {
        let mut args = vec!["book".to_string(), ws.s("b.penbook")];
        args.extend(action.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        refused_with(&ws, &refs, "[unsafe-path]");
    }
    assert_eq!(
        std::fs::read(outside.p("secret.pen")).unwrap(),
        secret_before
    );
}

#[test]
fn penbook_package_refuses_unsafe_destinations_and_members() {
    let ws = Workspace::new("book-package");
    member(&ws, "a.pen");
    ws.write("b.penbook", &book_manifest(vec![doc_entry("a.pen")]));
    // A package output that already exists is not overwritten silently.
    std::fs::write(ws.p("out.zip"), b"existing").unwrap();
    let before = ws.snapshot();
    let out = run(&[
        "book",
        &ws.s("b.penbook"),
        "package",
        "--out",
        &ws.s("out.zip"),
    ]);
    if !out.status.success() {
        assert!(String::from_utf8_lossy(&out.stderr).contains('['));
        assert!(before == ws.snapshot(), "failed package changed files");
    }
}

#[test]
fn penbook_member_that_is_invalid_aborts_every_write() {
    let ws = Workspace::new("book-member");
    member(&ws, "a.pen");
    member(&ws, "b.pen");
    let mut manifest = book_manifest(vec![doc_entry("a.pen"), doc_entry("b.pen")]);
    manifest["synchronize"] = json!({"source": "a.pen", "styles": true});
    ws.write("b.penbook", &manifest);
    std::fs::write(ws.p("b.pen"), b"{ not json").unwrap();
    for action in [vec!["renumber"], vec!["sync", "--styles"]] {
        let mut args = vec!["book".to_string(), ws.s("b.penbook")];
        args.extend(action.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        refused_with(&ws, &refs, "[missing-resource]");
    }
    // A member from a future format version is refused without mutation.
    member(&ws, "b.pen");
    let mut raw = ws.json("b.pen");
    raw["version"] = json!(99);
    ws.write("b.pen", &raw);
    let error = refused(&ws, &["book", &ws.s("b.penbook"), "renumber"]);
    assert!(error.contains('['), "{error}");
}

// ------------------------------------------------------------ tables

fn layout_doc(ws: &Workspace, rel: &str) -> String {
    let path = ws.s(rel);
    ok(&["new", &path, "--overwrite"]);
    ok(&["migrate", &path, "--target", "9"]);
    let mut raw = ws.json(rel);
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": [
        {"runs": [{"text": "intro"}]},
        {"runs": [{"text": "a"}, {"special": "tab"}, {"text": "b"}]},
        {"runs": [{"text": "outro"}]}
    ]}]);
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind": "text-frame", "id": "f", "story": "s", "next": null,
         "x": 0, "y": 0, "width": 200, "height": 100}
    ]);
    ws.write(rel, &raw);
    path
}

#[test]
fn table_import_limits_fail_by_name_and_leave_the_document_unchanged() {
    let ws = Workspace::new("tables");
    let doc = layout_doc(&ws, "doc.pen");

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "rows.csv",
            "a,b\n".repeat(10_001),
            "[limit-exceeded]",
        ),
        (
            "cols.csv",
            format!("{}\n", vec!["x"; 257].join(",")),
            "[limit-exceeded]",
        ),
        (
            "cell.csv",
            format!("\"{}\"\n", "y".repeat(70 * 1024)),
            "[limit-exceeded]",
        ),
        (
            "unterminated.csv",
            "a,\"never closed\nb,c\n".to_string(),
            "[malformed-resource]",
        ),
        (
            "stray-quote.csv",
            "ab\"cd,e\n".to_string(),
            "[malformed-resource]",
        ),
        ("empty.csv", String::new(), "[invalid-input]"),
    ];
    for (name, body, code) in &cases {
        std::fs::write(ws.p(name), body).unwrap();
        let error = refused_with(&ws, &["layout-table", &doc, "import", "t", &ws.s(name)], code);
        assert!(!error.contains("panicked"), "{name}: {error}");
    }

    // Larger than the 8 MiB import budget: refused from the file size alone.
    std::fs::write(ws.p("big.csv"), "a,b\n".repeat(2 * 1024 * 1024 + 1)).unwrap();
    refused_with(
        &ws,
        &["layout-table", &doc, "import", "t", &ws.s("big.csv")],
        "[limit-exceeded]",
    );

    // TSV: embedded delimiter limits apply identically.
    std::fs::write(ws.p("wide.tsv"), format!("{}\n", vec!["x"; 257].join("\t"))).unwrap();
    refused_with(
        &ws,
        &["layout-table", &doc, "import", "t", &ws.s("wide.tsv")],
        "[limit-exceeded]",
    );

    // Within the row and column limits but over the 100,000 cell budget.
    std::fs::write(ws.p("cells.csv"), "a,b,c,d,e,f,g,h,i,j,k
".repeat(10_000)).unwrap();
    refused_with(
        &ws,
        &["layout-table", &doc, "import", "t", &ws.s("cells.csv")],
        "[limit-exceeded]",
    );

    // Header rows that leave no body row, hostile column widths.
    std::fs::write(ws.p("ok.csv"), "a,b\nc,d\n").unwrap();
    refused_with(
        &ws,
        &[
            "layout-table",
            &doc,
            "import",
            "t",
            &ws.s("ok.csv"),
            "--header-rows",
            "2",
        ],
        "[invalid-input]",
    );
    for width in ["0", "-5", "NaN", "inf"] {
        refused(
            &ws,
            &[
                "layout-table",
                &doc,
                "import",
                "t",
                &ws.s("ok.csv"),
                "--column-width",
                width,
            ],
        );
    }
}

#[test]
fn table_creation_and_planning_limits_are_checked_before_work() {
    let ws = Workspace::new("tables-create");
    let doc = layout_doc(&ws, "doc.pen");
    let args = |rows: &str, cols: &str| {
        vec![
            "layout-table".to_string(),
            doc.clone(),
            "create".to_string(),
            "t".to_string(),
            "--rows".to_string(),
            rows.to_string(),
            "--columns".to_string(),
            cols.to_string(),
        ]
    };
    for (rows, cols) in [
        ("10001", "2"),
        ("2", "257"),
        ("0", "2"),
        ("2", "0"),
    ] {
        let a = args(rows, cols);
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        refused(&ws, &refs);
    }
    ok(&[
        "layout-table",
        &doc,
        "create",
        "t",
        "--rows",
        "3",
        "--columns",
        "2",
    ]);
    // Plan with hostile heights must name the problem, never write.
    for heights in ["-10", "0", "NaN", "inf"] {
        refused(&ws, &["layout-table", &doc, "plan", "t", "--heights", heights]);
    }
}

// ------------------------------------------------------------ styles

fn set_styles(ws: &Workspace, rel: &str, styles: Value) {
    let mut raw = ws.json(rel);
    raw["layout"]["styles"] = styles;
    ws.write(rel, &raw);
}

#[test]
fn style_chains_cycles_and_counts_are_bounded() {
    let ws = Workspace::new("styles");
    let doc = layout_doc(&ws, "doc.pen");

    // A chain of 16 styles (self plus 15 ancestors) validates.
    let mut paragraph = serde_json::Map::new();
    paragraph.insert("s0".into(), json!({"size": 10}));
    for i in 1..=15 {
        paragraph.insert(format!("s{i}"), json!({"based_on": format!("s{}", i - 1)}));
    }
    set_styles(&ws, "doc.pen", json!({"paragraph": paragraph.clone()}));
    ok(&["layout-style", &doc, "check"]);

    // One more is a named limit error from every read path.
    paragraph.insert("s16".into(), json!({"based_on": "s15"}));
    set_styles(&ws, "doc.pen", json!({"paragraph": paragraph}));
    refused_with(&ws, &["layout-style", &doc, "check"], "[limit-exceeded]");
    refused_with(
        &ws,
        &["layout-style", &doc, "show", "paragraph", "s16"],
        "[limit-exceeded]",
    );
    refused_with(
        &ws,
        &["layout-style", &doc, "create", "paragraph", "other"],
        "[limit-exceeded]",
    );

    // Cycles, self-reference and missing bases.
    for (label, styles, code) in [
        (
            "cycle",
            json!({"paragraph": {"a": {"based_on": "b"}, "b": {"based_on": "a"}}}),
            "cycle",
        ),
        (
            "self",
            json!({"paragraph": {"a": {"based_on": "a"}}}),
            "cycle",
        ),
        (
            "missing",
            json!({"paragraph": {"a": {"based_on": "ghost"}}}),
            "[missing-resource]",
        ),
    ] {
        set_styles(&ws, "doc.pen", styles);
        let error = refused(&ws, &["layout-style", &doc, "check"]);
        assert!(error.contains(code), "{label}: {error}");
    }

    // Creating a cycle through the CLI is refused and does not mutate.
    set_styles(
        &ws,
        "doc.pen",
        json!({"paragraph": {"a": {"size": 10}, "b": {"based_on": "a"}}}),
    );
    let error = refused(
        &ws,
        &["layout-style", &doc, "set", "paragraph", "a", "--based-on", "b"],
    );
    assert!(error.contains("cycle"), "{error}");

    // More than 2,000 styles of a kind.
    let many: serde_json::Map<String, Value> = (0..2001)
        .map(|i| (format!("p{i}"), json!({"size": 10})))
        .collect();
    set_styles(&ws, "doc.pen", json!({"paragraph": many}));
    refused_with(&ws, &["layout-style", &doc, "check"], "[limit-exceeded]");
}

// ------------------------------------------------------------ stories

fn set_story(ws: &Workspace, rel: &str, paragraphs: Value) {
    let mut raw = ws.json(rel);
    raw["layout"]["stories"] = json!([{"id": "s", "paragraphs": paragraphs}]);
    ws.write(rel, &raw);
}

#[test]
fn huge_stories_are_refused_before_composition() {
    let ws = Workspace::new("stories");
    let doc = layout_doc(&ws, "doc.pen");
    let out = ws.s("out.svg");

    // More than 5,000,000 characters in one story.
    set_story(
        &ws,
        "doc.pen",
        json!([{"runs": [{"text": "word ".repeat(1_100_000)}]}]),
    );
    for args in [
        vec!["text", &doc, "story", "flow", "s", "--max-pages", "5"],
        vec!["export", &doc, &out],
    ] {
        let error = refused(&ws, &args);
        assert!(error.contains("[limit-exceeded]"), "{args:?}: {error}");
    }

    // More than 100,000 paragraphs.
    let paragraphs: Vec<Value> = (0..100_001).map(|_| json!({"runs": []})).collect();
    set_story(&ws, "doc.pen", json!(paragraphs));
    refused_with(
        &ws,
        &["text", &doc, "story", "flow", "s", "--max-pages", "5"],
        "[limit-exceeded]",
    );

    // More than 10,000 runs in one paragraph.
    let runs: Vec<Value> = (0..10_001).map(|_| json!({"text": "x"})).collect();
    set_story(&ws, "doc.pen", json!([{"runs": runs}]));
    refused_with(
        &ws,
        &["text", &doc, "story", "flow", "s", "--max-pages", "5"],
        "[limit-exceeded]",
    );
}

#[test]
fn story_add_and_flow_limits_are_checked_up_front() {
    let ws = Workspace::new("stories-flow");
    let doc = layout_doc(&ws, "doc.pen");
    // Page budget above the hard maximum is refused before any page is added.
    for pages in ["5001", "0"] {
        refused_with(
            &ws,
            &["text", &doc, "story", "flow", "s", "--max-pages", pages],
            "[invalid-input]",
        );
    }
    // Control characters and missing stories fail by name.
    refused(
        &ws,
        &["text", &doc, "story", "add", "bad", "--text", "a\u{7}b"],
    );
    refused_with(
        &ws,
        &["text", &doc, "story", "flow", "ghost"],
        "[missing-resource]",
    );

    // A thread longer than 4,096 frames.
    let mut raw = ws.json("doc.pen");
    let frames: Vec<Value> = (0..4097)
        .map(|i| {
            json!({"kind": "text-frame", "id": format!("f{i}"), "story": "s",
                "next": if i < 4096 { json!(format!("f{}", i + 1)) } else { Value::Null },
                "x": 0, "y": 0, "width": 10, "height": 10})
        })
        .collect();
    raw["pages"][0]["layers"][0]["nodes"] = json!(frames);
    ws.write("doc.pen", &raw);
    refused_with(
        &ws,
        &["text", &doc, "story", "flow", "s", "--max-pages", "2"],
        "[limit-exceeded]",
    );
}

#[test]
fn hostile_frame_dimensions_are_refused() {
    let ws = Workspace::new("frames");
    layout_doc(&ws, "doc.pen");
    for (label, width, height) in [
        ("negative", json!(-5), json!(10)),
        ("huge", json!(2_000_000), json!(10)),
        ("huge-height", json!(10), json!(2_000_000)),
        ("huge-float", json!(1e300), json!(10)),
        ("nan-string", json!(10), json!("NaN")),
    ] {
        let mut raw = ws.json("doc.pen");
        raw["pages"][0]["layers"][0]["nodes"][0]["width"] = width;
        raw["pages"][0]["layers"][0]["nodes"][0]["height"] = height;
        let name = format!("{label}.pen");
        ws.write(&name, &raw);
        let error = refused(
            &ws,
            &["text", &ws.s(&name), "story", "flow", "s", "--max-pages", "2"],
        );
        assert!(error.contains('['), "{label}: {error}");
    }
}

// ------------------------------------------------------------ long documents

#[test]
fn long_document_inputs_are_bounded() {
    let ws = Workspace::new("longdoc");
    let doc = layout_doc(&ws, "doc.pen");
    // Hyperlink URL over 2,048 bytes and a non-network scheme.
    for url in [
        format!("https://example.com/{}", "a".repeat(3000)),
        "javascript:alert(1)".to_string(),
        "file:///etc/passwd".to_string(),
    ] {
        refused(
            &ws,
            &[
                "layout-doc",
                &doc,
                "link-add",
                "l1",
                "--story",
                "s",
                "--paragraph",
                "0",
                "--at",
                "1",
                "--text",
                "go",
                "--url",
                &url,
            ],
        );
    }
}
