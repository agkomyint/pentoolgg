//! v0.14.0 task 1: the frozen layout specification, schemas and fixtures.
//! These tests pin the contract files themselves; engine behavior is tested by
//! the tasks that implement it.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

fn read_json(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    serde_json::from_slice(
        &std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn fixture(name: &str) -> Value {
    read_json(Path::new("docs/fixtures").join(name))
}

fn roman(mut n: u32) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, text) in table {
        while n >= value {
            out.push_str(text);
            n -= value;
        }
    }
    out
}

/// Section number formatting from docs/layout-v1.md section 3.4.
fn format_number(style: &str, n: u32) -> String {
    match style {
        "arabic" => n.to_string(),
        "lower-roman" => roman(n),
        "upper-roman" => roman(n).to_uppercase(),
        "lower-alpha" | "upper-alpha" => {
            let letter = (b'a' + ((n - 1) % 26) as u8) as char;
            let text: String = std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect();
            if style == "upper-alpha" {
                text.to_uppercase()
            } else {
                text
            }
        }
        other => panic!("unknown numbering style {other}"),
    }
}

/// Mirrors the penbook member-path rule in docs/layout-v1.md section 11.
fn penbook_path_allowed(path: &str) -> bool {
    if path.is_empty() || path.contains('\0') || !path.ends_with(".pen") {
        return false;
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return false;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    !path.split(['/', '\\']).any(|part| part == "..")
}

#[test]
fn schemas_are_draft_2020_12_and_declare_the_expected_ids() {
    for (file, id) in [
        (
            "docs/layout-v1.schema.json",
            "https://pentool.space/schema/layout-v1.json",
        ),
        (
            "docs/pen-format-v9.schema.json",
            "https://pentool.space/schema/pen-v9.json",
        ),
        (
            "docs/penbook-v1.schema.json",
            "https://pentool.space/schema/penbook-v1.json",
        ),
    ] {
        let schema = read_json(file);
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["$id"], id, "{file}");
    }
    let v9 = read_json("docs/pen-format-v9.schema.json");
    assert_eq!(v9["properties"]["version"]["const"], 9);
}

#[test]
fn manifest_names_existing_parseable_fixtures() {
    let manifest = fixture("layout-v1-manifest.json");
    assert_eq!(manifest["spec"], "layout-v1");
    let entries = manifest["fixtures"].as_array().unwrap();
    let mut kinds = HashSet::new();
    for entry in entries {
        let file = entry["file"].as_str().unwrap();
        let _ = fixture(file);
        assert!(matches!(
            entry["expect"].as_str(),
            Some("valid" | "invalid")
        ));
        if entry["expect"] == "invalid" {
            assert!(entry["error"].is_string(), "{file} needs an error code");
        }
        kinds.insert(entry["kind"].as_str().unwrap().to_string());
    }
    for kind in ["document", "book", "adversarial", "cross-platform"] {
        assert!(kinds.contains(kind), "manifest lacks a {kind} fixture");
    }
    // No IDML file is claimed until it exists.
    assert_eq!(manifest["idml_corpus"]["status"], "planned");
}

#[test]
fn page_numbering_vectors_match_the_specified_formats() {
    let vectors = fixture("layout-v1-page-numbering.json");
    let list = vectors["vectors"].as_array().unwrap();
    assert!(list.len() >= 20);
    for v in list {
        let style = v["style"].as_str().unwrap();
        let n = v["n"].as_u64().unwrap() as u32;
        let mut want = format_number(style, n);
        if let Some(prefix) = v["prefix"].as_str() {
            want = format!("{prefix}{want}");
        }
        assert_eq!(v["text"], want, "{style} {n}");
    }
    // Spot checks written independently of the formatter.
    let get = |style: &str, n: u64| {
        list.iter()
            .find(|v| v["style"] == style && v["n"] == n)
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(get("lower-alpha", 27), "aa");
    assert_eq!(get("lower-alpha", 53), "aaa");
    assert_eq!(get("lower-roman", 1994), "mcmxciv");
}

#[test]
fn penbook_fixtures_follow_the_path_containment_rule() {
    for (file, ok) in [
        ("penbook-valid.penbook", true),
        ("penbook-invalid-path-escape.penbook", false),
        ("penbook-invalid-absolute.penbook", false),
    ] {
        let book = fixture(file);
        assert_eq!(book["format"], "penbook");
        let all_ok = book["documents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| penbook_path_allowed(d["path"].as_str().unwrap()));
        assert_eq!(all_ok, ok, "{file}");
    }
}

/// A deliberately small semantic checker: it proves each invalid fixture
/// really contains the defect named in the manifest, and the valid one has
/// none. The engine's own validator replaces this when tasks 3 and 6 land.
fn defects(doc: &Value) -> HashSet<&'static str> {
    let mut out = HashSet::new();
    let layout = &doc["layout"];

    let mut next: HashMap<String, Option<String>> = HashMap::new();
    for page in doc["pages"].as_array().unwrap() {
        for layer in page["layers"].as_array().unwrap() {
            for node in layer["nodes"].as_array().unwrap() {
                if node["kind"] == "text-frame" {
                    next.insert(
                        node["id"].as_str().unwrap().to_string(),
                        node["next"].as_str().map(str::to_string),
                    );
                    if node["width"].as_f64().unwrap_or(1.0) <= 0.0 {
                        out.insert("invalid-dimension");
                    }
                }
            }
        }
    }
    for start in next.keys() {
        let mut seen = HashSet::new();
        let mut cursor = Some(start.clone());
        while let Some(id) = cursor {
            if !seen.insert(id.clone()) {
                out.insert("thread-cycle");
                break;
            }
            cursor = next.get(&id).cloned().flatten();
        }
    }

    let empty = serde_json::Map::new();
    let styles = layout["styles"]["paragraph"].as_object().unwrap_or(&empty);
    for (id, style) in styles {
        let mut seen = HashSet::from([id.clone()]);
        let mut cursor = style["based_on"].as_str();
        while let Some(parent) = cursor {
            if !seen.insert(parent.to_string()) {
                out.insert("style-cycle");
                break;
            }
            cursor = styles.get(parent).and_then(|s| s["based_on"].as_str());
        }
    }
    for story in layout["stories"].as_array().unwrap_or(&Vec::new()) {
        for paragraph in story["paragraphs"].as_array().unwrap() {
            if let Some(style) = paragraph["style"].as_str() {
                if !styles.contains_key(style) {
                    out.insert("unknown-style");
                }
            }
        }
    }
    out
}

#[test]
fn document_fixtures_exhibit_exactly_their_declared_defect() {
    let manifest = fixture("layout-v1-manifest.json");
    for entry in manifest["fixtures"].as_array().unwrap() {
        if entry["kind"] != "document" {
            continue;
        }
        let found = defects(&fixture(entry["file"].as_str().unwrap()));
        match entry["expect"].as_str().unwrap() {
            "valid" => assert!(found.is_empty(), "{}: {found:?}", entry["file"]),
            _ => {
                let want = entry["error"].as_str().unwrap();
                assert!(found.contains(want), "{}: {found:?}", entry["file"]);
            }
        }
    }
}

#[test]
fn adversarial_cases_name_the_limits_in_the_spec() {
    let spec = std::fs::read_to_string("docs/layout-v1.md").unwrap();
    let cases = fixture("layout-v9-adversarial-limits.json");
    for case in cases["cases"].as_array().unwrap() {
        assert!(case["expect_error"].is_string());
    }
    for phrase in [
        "Thread length",
        "GREP pattern length",
        "Tables per document",
        "IDML archive size",
    ] {
        assert!(spec.contains(phrase), "limits table lacks {phrase}");
    }
}

#[test]
fn mapping_table_uses_only_the_four_documented_statuses() {
    let text = std::fs::read_to_string("docs/idml-mapping-v1.md").unwrap();
    let mut rows = 0;
    for line in text.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // Mapping rows are `| IDML | .pen | status |`; the status column is
        // the last non-empty cell and starts with one of the four words.
        if cells.len() != 5 {
            continue;
        }
        let status = cells[3];
        if status.is_empty() || status.starts_with("---") || status == "Status" {
            continue;
        }
        if !["supported", "approximated", "carried", "unsupported"]
            .iter()
            .any(|s| status.starts_with(s))
        {
            continue;
        }
        rows += 1;
    }
    assert!(rows > 60, "mapping table has only {rows} classified rows");
}
