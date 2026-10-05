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

#[test]
fn jpeg_exif_orientation_is_applied_and_recorded() {
    let source = ::image::DynamicImage::new_rgb8(2, 3);
    let mut encoded = std::io::Cursor::new(Vec::new());
    source
        .write_to(&mut encoded, ::image::ImageFormat::Jpeg)
        .unwrap();
    let payload = [
        b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0,
        0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let length = u16::try_from(payload.len() + 2).unwrap().to_be_bytes();
    let original = encoded.into_inner();
    let mut oriented = Vec::with_capacity(original.len() + payload.len() + 4);
    oriented.extend_from_slice(&original[..2]);
    oriented.extend_from_slice(&[0xff, 0xe1, length[0], length[1]]);
    oriented.extend_from_slice(&payload);
    oriented.extend_from_slice(&original[2..]);

    let info = image::decode_source(&oriented).unwrap();
    assert_eq!(info.orientation, 6);
    assert_eq!((info.pixel_width, info.pixel_height), (3, 2));
}

#[test]
fn vector_masks_validate_and_render_in_parent_space() {
    let mut document = fixture("valid");
    document["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"kind":"path","id":"mask-shape","d":"M0 0 L1 0 L1 1 L0 1 Z","transform":[1,0,0,1,0,0]}),
        );
    document["pages"][0]["layers"][0]["nodes"][1]["mask"] =
        json!({"node":"mask-shape","space":"parent","fill_rule":"evenodd"});
    transaction::validate_value(&document).unwrap();
    let rendered = image::to_svg(&document, std::path::Path::new("fixture.pen"), None).unwrap();
    assert!(rendered.svg.contains("vector-mask-"));
    assert!(rendered.svg.contains("fill-rule=\"evenodd\""));

    document["pages"][0]["layers"][0]["nodes"][1]["mask"]["node"] = json!("missing");
    assert!(transaction::validate_value(&document)
        .unwrap_err()
        .to_string()
        .contains("[missing-mask]"));
}

#[test]
fn external_assets_relocate_and_changed_bytes_fail_before_render() {
    let unique = format!(
        "pentool-v070-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let first = std::env::temp_dir().join(&unique).join("first");
    let second = std::env::temp_dir().join(&unique).join("second");
    std::fs::create_dir_all(first.join("assets")).unwrap();
    let source = ::image::DynamicImage::new_rgba8(2, 2);
    let mut encoded = std::io::Cursor::new(Vec::new());
    source
        .write_to(&mut encoded, ::image::ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    std::fs::write(first.join("assets/photo.png"), &bytes).unwrap();
    let mut document: Value =
        serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
    image::add(
        &mut document,
        None,
        "content",
        "external-photo",
        &bytes,
        image::external_storage(std::path::Path::new("assets/photo.png")).unwrap(),
        0.0,
        0.0,
        20.0,
        20.0,
        image::Fit::Contain,
    )
    .unwrap();
    std::fs::write(
        first.join("scene.pen"),
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(second.parent().unwrap()).unwrap();
    std::fs::rename(&first, &second).unwrap();
    image::to_svg(&document, &second.join("scene.pen"), None).unwrap();

    std::fs::write(second.join("assets/photo.png"), b"changed").unwrap();
    assert!(image::to_svg(&document, &second.join("scene.pen"), None)
        .unwrap_err()
        .to_string()
        .contains("[hash-mismatch]"));
    std::fs::remove_dir_all(std::env::temp_dir().join(unique)).unwrap();
}

#[test]
fn embedded_images_deduplicate_edit_render_and_prune_atomically() {
    let source = ::image::DynamicImage::new_rgba8(4, 2);
    let mut cursor = std::io::Cursor::new(Vec::new());
    source
        .write_to(&mut cursor, ::image::ImageFormat::Png)
        .unwrap();
    let bytes = cursor.into_inner();
    let mut document: Value =
        serde_json::from_str(include_str!("../docs/fixtures/v4-scene.pen")).unwrap();
    let first = image::add(
        &mut document,
        None,
        "content",
        "photo-a",
        &bytes,
        image::embedded_storage(&bytes),
        20.0,
        20.0,
        160.0,
        90.0,
        image::Fit::Cover,
    )
    .unwrap();
    let second = image::add(
        &mut document,
        None,
        "content",
        "photo-b",
        &bytes,
        image::embedded_storage(&bytes),
        40.0,
        40.0,
        80.0,
        80.0,
        image::Fit::Contain,
    )
    .unwrap();
    assert!(!first.deduplicated);
    assert!(second.deduplicated);
    assert_eq!(document["image_assets"].as_object().unwrap().len(), 1);

    image::set(
        &mut document,
        None,
        "photo-b",
        &image::Update {
            crop: Some([0.25, 0.0, 0.5, 1.0]),
            opacity: Some(0.75),
            ..Default::default()
        },
    )
    .unwrap();
    let scene = image::to_svg(&document, std::path::Path::new("fixture.pen"), None).unwrap();
    assert!(scene.svg.contains("data:image/png;base64,"));
    let png = pentool::render::svg_to_png(&scene.svg, scene.width, scene.height, 1.0).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    image::remove(&mut document, None, "photo-a").unwrap();
    assert_eq!(document["image_assets"].as_object().unwrap().len(), 1);
    image::remove(&mut document, None, "photo-b").unwrap();
    assert!(document["image_assets"].as_object().unwrap().is_empty());
}
