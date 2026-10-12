use serde_json::Value;
use std::path::{Path, PathBuf};

fn read_json(path: impl AsRef<Path>) -> Value {
    let path = path.as_ref();
    serde_json::from_slice(
        &std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

#[test]
fn agent_painting_schemas_are_frozen_draft_2020_12_contracts() {
    for (file, expected_id) in [
        (
            "docs/agent-painting-request-v1.schema.json",
            "https://pentool.space/schema/agent-painting-request-v1.json",
        ),
        (
            "docs/paint-plan-v1.schema.json",
            "https://pentool.space/schema/paint-plan-v1.json",
        ),
        (
            "docs/agent-painting-result-v1.schema.json",
            "https://pentool.space/schema/agent-painting-result-v1.json",
        ),
    ] {
        let schema = read_json(file);
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["$id"], expected_id);
        assert_eq!(schema["additionalProperties"], false);
    }
}

#[test]
fn agent_painting_manifest_names_parseable_bounded_fixtures() {
    let root = PathBuf::from("docs/fixtures");
    let manifest = read_json(root.join("agent-painting-v1-manifest.json"));
    let fixtures = manifest["fixtures"].as_array().unwrap();
    assert_eq!(fixtures.len(), 6);

    for fixture in fixtures {
        let name = fixture["file"].as_str().unwrap();
        assert!(!name.contains(['/', '\\']));
        let value = read_json(root.join(name));
        assert_eq!(value["schema"], 1, "{name}");
        assert!(std::fs::metadata(root.join(name)).unwrap().len() <= 4 * 1024 * 1024);
    }
}

#[test]
fn invalid_fixtures_capture_semantic_and_preallocation_failures() {
    let curve = read_json("docs/fixtures/agent-painting-v1-invalid-curve.json");
    let points = curve["curves"]["opacity"].as_array().unwrap();
    assert!(points
        .windows(2)
        .any(|pair| { pair[0][0].as_f64().unwrap() >= pair[1][0].as_f64().unwrap() }));

    let attack = read_json("docs/fixtures/agent-painting-v1-adversarial-budget.json");
    assert!(attack["passes"].as_array().unwrap().is_empty());
    assert!(attack["limits"]["max_dabs"].as_u64().unwrap() > 4_000_000);
    assert!(attack["limits"]["max_changed_pixels"].as_u64().unwrap() > 67_108_864);
}

#[test]
fn cross_platform_vector_pins_endpoint_and_rounding_rules() {
    let vector = read_json("docs/fixtures/agent-painting-v1-cross-platform.json");
    assert_eq!(
        vector["expected"]["normalized_points"],
        serde_json::json!([[0, 0], [1, 1]])
    );
    assert_eq!(vector["expected"]["source_samples"], 2);
    assert_eq!(vector["expected"]["generated_samples"], 2);
    assert_eq!(
        vector["expected"]["canonical_samples"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        vector["expected"]["endpoint_policy"],
        "first-and-final-exactly-once"
    );
    assert_eq!(
        vector["expected"]["rounding"],
        "nearest-0.001-ties-away-from-zero"
    );
}
