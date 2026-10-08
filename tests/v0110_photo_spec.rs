//! Photography specification (v0.11.0 item 1): schema, fixtures and the
//! pre-implementation compatibility contract in `docs/photography-v1.md`.
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn manifest() -> Value {
    read_json(Path::new("docs/fixtures/photo-conformance.json"))
}

fn fixture(name: &str) -> PathBuf {
    Path::new("docs/fixtures").join(name)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn photo_schema_references_resolve_to_tracked_definitions() {
    fn walk(schema: &Value, file: &Path) {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let (relative, pointer) = reference.split_once('#').unwrap();
            let target = if relative.is_empty() {
                file.to_owned()
            } else {
                file.parent().unwrap().join(relative)
            };
            assert!(
                read_json(&target).pointer(pointer).is_some(),
                "{}: {reference}",
                file.display()
            );
        }
        match schema {
            Value::Object(fields) => fields.values().for_each(|child| walk(child, file)),
            Value::Array(values) => values.iter().for_each(|child| walk(child, file)),
            _ => {}
        }
    }
    let path = Path::new("docs/pen-format-v7.schema.json");
    let schema = read_json(path);
    assert_eq!(schema["properties"]["version"]["const"], 7);
    walk(&schema, path);
}

#[test]
fn conformance_fixtures_match_their_pinned_digests() {
    let manifest = manifest();
    let fixtures = manifest["fixtures"].as_array().unwrap();
    assert!(!fixtures.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for entry in fixtures {
        let name = entry["file"].as_str().unwrap();
        assert!(seen.insert(name), "{name} listed twice");
        assert!(
            name.starts_with("photo-"),
            "{name} must use the photo- prefix"
        );
        let bytes = std::fs::read(fixture(name)).unwrap();
        assert!(
            bytes.len() <= 64 * 1024,
            "{name} must stay a minimal fixture"
        );
        assert_eq!(
            entry["sha256"].as_str().unwrap(),
            sha256(&bytes),
            "{name} changed; regenerate it deliberately and update the manifest"
        );
        match entry["expect"].as_str().unwrap() {
            "ok" => assert!(entry.get("code").is_none(), "{name}"),
            "error" => assert!(entry["code"].is_string(), "{name} needs an error code"),
            other => panic!("{name}: unknown expectation {other}"),
        }
    }
    // Every tracked photo fixture is listed, so none can drift unpinned.
    for file in std::fs::read_dir("docs/fixtures").unwrap() {
        let name = file.unwrap().file_name().into_string().unwrap();
        if name.starts_with("photo-") && name != "photo-conformance.json" {
            assert!(
                seen.contains(name.as_str()),
                "{name} is not in the manifest"
            );
        }
    }
}

#[test]
fn v7_fixture_assets_are_content_addressed_and_within_limits() {
    let doc = read_json(&fixture("photo-v7.pen"));
    assert_eq!(doc["version"], 7);
    assert_eq!(doc["compositing"]["color_space"], "srgb8");
    let photography = &doc["photography"];
    assert_eq!(photography["engine"], 1);
    assert_eq!(photography["working_space"], "prophoto-linear");
    let assets = photography["assets"].as_object().unwrap();
    for (digest, asset) in assets {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(asset["storage"]["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(digest, &format!("sha256:{}", sha256(&bytes)));
        assert_eq!(asset["byte_length"].as_u64().unwrap(), bytes.len() as u64);
        assert!(bytes.len() as u64 <= pentool::image::MAX_SOURCE_BYTES);
        // Each embedded source is itself a pinned DNG fixture.
        assert!(manifest()["fixtures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["sha256"] == sha256(&bytes)
                && entry["file"].as_str().unwrap().ends_with(".dng")));
    }
    for photo in photography["photos"].as_array().unwrap() {
        assert!(assets.contains_key(photo["source"].as_str().unwrap()));
        assert_eq!(photo["variants"][0]["id"], "master");
    }
}

#[test]
fn current_build_refuses_v7_documents_without_mutation() {
    let root = std::env::temp_dir().join(format!(
        "pentool-photo-spec-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for entry in manifest()["fixtures"].as_array().unwrap() {
        let name = entry["file"].as_str().unwrap();
        if !name.ends_with(".pen") {
            continue;
        }
        let raw = read_json(&fixture(name));
        let error = pentool::transaction::validate_value(&raw)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("[unsupported-capability]") && error.contains("version 7"),
            "{name}: {error}"
        );
        let path = root.join(name);
        std::fs::copy(fixture(name), &path).unwrap();
        let before = std::fs::read(&path).unwrap();
        for args in [
            vec!["info"],
            vec!["migrate", "--target", "6"],
            vec!["migrate", "--target", "4"],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_pentool"))
                .args(&args)
                .arg(&path)
                .output()
                .unwrap();
            assert!(!output.status.success(), "{name}: {args:?} succeeded");
            assert_eq!(std::fs::read(&path).unwrap(), before, "{name}: {args:?}");
        }
        assert!(!root.join(".pentool").exists(), "{name}: history written");
    }
    std::fs::remove_dir_all(root).unwrap();
}
