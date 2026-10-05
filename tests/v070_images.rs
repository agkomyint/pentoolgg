use pentool::{image, scene, transaction};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let source = match name {
        "valid" => include_str!("../docs/fixtures/v5-image-embedded.pen"),
        "hash" => include_str!("../docs/fixtures/v5-image-invalid-hash.pen"),
        "path" => include_str!("../docs/fixtures/v5-image-invalid-path.pen"),
        "node" => include_str!("../docs/fixtures/v5-image-invalid-node.pen"),
        _ => unreachable!(),
    };
    serde_json::from_str(source).unwrap()
}

#[test]
fn v5_normative_image_fixtures_validate_or_fail_before_use() {
    transaction::validate_value(&fixture("valid")).unwrap();
    assert!(transaction::validate_value(&fixture("hash"))
        .unwrap_err()
        .to_string()
        .contains("[hash-mismatch]"));
    assert!(transaction::validate_value(&fixture("path"))
        .unwrap_err()
        .to_string()
        .contains("[unsafe-path]"));
    let error = transaction::validate_value(&fixture("node"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("[missing-resource]") || error.contains("[malformed-resource]"));
}

#[test]
fn v4_to_v5_migration_is_explicit_and_preserves_extensions() {
    let v4: Value = serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
    let mut with_extension = v4.clone();
    with_extension["x-test"] = json!({"keep": true});
    let v5 = scene::migrate_to_v5(with_extension).unwrap();
    assert_eq!(v5["version"], 5);
    assert_eq!(v5["image_assets"], json!({}));
    assert_eq!(v5["x-test"], json!({"keep": true}));
    transaction::validate_value(&v5).unwrap();

    let round_trip = scene::migrate_to_v4(v5).unwrap();
    assert_eq!(round_trip["version"], 4);
    assert!(round_trip.get("image_assets").is_none());
    assert_eq!(round_trip["x-test"], json!({"keep": true}));
}

#[test]
fn image_content_has_an_actionable_v3_downgrade_error() {
    let error = scene::flatten_to_v3(&fixture("valid"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot represent image node pixel"));
}

#[test]
fn bounded_decoder_accepts_png_jpeg_and_webp_signatures() {
    for (format, media_type) in [
        (::image::ImageFormat::Png, "image/png"),
        (::image::ImageFormat::Jpeg, "image/jpeg"),
        (::image::ImageFormat::WebP, "image/webp"),
    ] {
        let source = ::image::DynamicImage::new_rgb8(2, 3);
        let mut cursor = std::io::Cursor::new(Vec::new());
        source.write_to(&mut cursor, format).unwrap();
        let info = image::decode_source(cursor.get_ref()).unwrap();
        assert_eq!(info.media_type, media_type);
        assert_eq!((info.pixel_width, info.pixel_height), (2, 3));
        assert!(info.digest.starts_with("sha256:"));
    }

    assert!(image::decode_source(b"not an image")
        .unwrap_err()
        .to_string()
        .contains("[malformed-resource]"));
}
