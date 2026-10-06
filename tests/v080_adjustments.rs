use pentool::{composite, scene, transaction};
use serde_json::{json, Value};
use std::path::Path;

fn rect(id: &str, x: u32, width: u32, color: &str) -> Value {
    json!({"kind":"rect","id":id,"x":x,"y":0,"width":width,"height":4,
        "style":{"fill":{"fallback":color}}})
}
fn document() -> Value {
    let mut raw = composite::migrate(scene::new_document(8, 4)).unwrap();
    raw["pages"][0]["canvas"]["background"] = json!("none");
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        rect("left", 0, 4, "#204060"),
        rect("right", 4, 4, "#80a0c0")
    ]);
    raw
}
fn add(raw: &mut Value, id: &str, kind: &str, scope: &str, params: Value) {
    composite::edit(
        raw,
        None,
        "add",
        id,
        &json!({"adjustment":kind,"params":params,
        "scope":composite::parse_scope(scope).unwrap()}),
    )
    .unwrap();
}

#[test]
fn adjustments_preserve_alpha_and_have_exact_reference_pixels() {
    let cases = [
        ("exposure", json!({"stops":1}), [128, 255, 255]),
        ("invert", json!({}), [191, 127, 63]),
        ("posterize", json!({"levels":2}), [0, 255, 255]),
        ("threshold", json!({"level":128}), [0, 0, 0]),
        (
            "color-balance",
            json!({"red":10,"blue":-10}),
            [90, 128, 167],
        ),
        ("black-and-white", json!({}), [119, 119, 119]),
        (
            "channel-mixer",
            json!({"matrix":[[0,0,1,0],[0,1,0,0],[1,0,0,0]]}),
            [192, 128, 64],
        ),
        (
            "gradient-map",
            json!({"stops":[{"position":0,"rgb":[0,0,0]},{"position":1,"rgb":[255,0,0]}]}),
            [119, 0, 0],
        ),
        ("vibrance", json!({"amount":-100}), [91, 124, 156]),
    ];
    for (kind, params, expected) in cases {
        let mut image = image::RgbaImage::from_pixel(1, 1, image::Rgba([64, 128, 192, 73]));
        composite::adjust(&mut image, kind, &params, 1.0).unwrap();
        assert_eq!(
            image.get_pixel(0, 0).0,
            [expected[0], expected[1], expected[2], 73],
            "{kind}"
        );
    }
    for kind in [
        "invert",
        "exposure",
        "vibrance",
        "black-and-white",
        "posterize",
        "threshold",
    ] {
        let mut image = image::RgbaImage::from_pixel(1, 1, image::Rgba([17, 31, 49, 0]));
        composite::adjust(&mut image, kind, &json!({}), 1.0).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [17, 31, 49, 0]);
    }
}

#[test]
fn identity_and_parameter_validation_cover_all_thirteen_kinds() {
    let identities = [
        ("exposure", json!({})),
        ("brightness-contrast", json!({})),
        ("levels", json!({})),
        ("curves", json!({"points":[[0,0],[255,255]]})),
        ("vibrance", json!({})),
        ("hue-saturation", json!({})),
        ("color-balance", json!({})),
        (
            "channel-mixer",
            json!({"matrix":[[1,0,0,0],[0,1,0,0],[0,0,1,0]]}),
        ),
        ("posterize", json!({"levels":256})),
    ];
    for (kind, params) in identities {
        let mut image = image::RgbaImage::new(256, 1);
        for (i, pixel) in image.pixels_mut().enumerate() {
            *pixel = image::Rgba([i as u8, i as u8, i as u8, 255]);
        }
        let before = image.clone();
        composite::adjust(&mut image, kind, &params, 1.0).unwrap();
        assert_eq!(before, image, "{kind}");
    }
    for (kind, params) in [
        ("exposure", json!({"stops":17})),
        ("vibrance", json!({"amount":101})),
        ("color-balance", json!({"red":-101})),
        ("black-and-white", json!({"red":1})),
        ("posterize", json!({"levels":2.5})),
        ("threshold", json!({"level":256})),
        ("channel-mixer", json!({"matrix":[[1,0,0,0]]})),
        ("gradient-map", json!({"stops":[]})),
        ("invert", json!({"typo":1})),
        ("levels", json!({"black":200,"white":100})),
        ("curves", json!({"points":[[0,0]]})),
        ("hue-saturation", json!({"hue":181})),
        ("brightness-contrast", json!({"contrast":101})),
    ] {
        assert!(
            composite::validate_settings(kind, &params).is_err(),
            "{kind}"
        );
    }
}

#[test]
fn scoped_and_global_adjustments_follow_stack_order_and_skip_foreground() {
    let mut raw = document();
    add(&mut raw, "global", "invert", "below", json!({}));
    add(&mut raw, "local", "invert", "ids:left", json!({}));
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(rect("top", 0, 1, "#ff0000"));
    let pixels = composite::render(&raw, Path::new("target/composite.pen"), None, 1.0).unwrap();
    assert_eq!(pixels.get_pixel(1, 1).0, [32, 64, 96, 255]);
    assert_eq!(pixels.get_pixel(5, 1).0, [127, 95, 63, 255]);
    assert_eq!(pixels.get_pixel(0, 1).0, [255, 0, 0, 255]);
    assert_eq!(raw["image_assets"], json!({}));
    let mut hidden = raw.clone();
    composite::edit(&mut hidden, None, "disable", "local", &json!({})).unwrap();
    assert_eq!(
        composite::render(&hidden, Path::new("target/composite.pen"), None, 1.0)
            .unwrap()
            .get_pixel(1, 1)
            .0,
        [223, 191, 159, 255]
    );
}

#[test]
fn group_scope_transforms_and_opacity_do_not_mutate_sources() {
    let mut raw = document();
    raw["pages"][0]["layers"][0]["nodes"] = json!([
        {"kind":"group","id":"hero","transform":[1,0,0,1,2,0],"children":[rect("child",0,2,"#204060")]}
    ]);
    add(&mut raw, "grade", "invert", "group:hero", json!({}));
    composite::edit(&mut raw, None, "set", "grade", &json!({"opacity":0.5})).unwrap();
    let before = raw.clone();
    let image = composite::render(&raw, Path::new("target/composite.pen"), None, 2.0).unwrap();
    assert_eq!(image.dimensions(), (16, 8));
    assert_eq!(image.get_pixel(5, 3).0, [128, 128, 128, 255]);
    assert_eq!(image.get_pixel(0, 3).0, [0, 0, 0, 0]);
    assert_eq!(raw, before);
}

#[test]
fn migration_locks_invalid_scopes_and_limits_fail_atomically() {
    let v4 = scene::new_document(8, 4);
    assert_eq!(
        scene::migrate_to_v4(composite::migrate(v4.clone()).unwrap()).unwrap(),
        v4
    );
    let mut raw = document();
    let before = raw.clone();
    assert!(composite::edit(
        &mut raw,
        None,
        "add",
        "bad",
        &json!({"adjustment":"invert","scope":{"kind":"targets","ids":["missing"]}})
    )
    .is_err());
    assert_eq!(raw, before);
    raw["pages"][0]["layers"][0]["locked"] = json!(true);
    let before = raw.clone();
    assert!(composite::edit(
        &mut raw,
        None,
        "add",
        "locked",
        &json!({"adjustment":"invert"})
    )
    .is_err());
    assert_eq!(raw, before);
    raw["pages"][0]["layers"][0]["locked"] = json!(false);
    add(&mut raw, "grade", "invert", "below", json!({}));
    assert!(scene::migrate_to_v5(raw.clone()).is_err());
    assert!(scene::flatten_to_v3(&raw).is_err());
    raw["pages"][0]["canvas"]["width"] = json!(16384);
    raw["pages"][0]["canvas"]["height"] = json!(16384);
    assert!(
        composite::render(&raw, Path::new("target/composite.pen"), None, 1.0)
            .unwrap_err()
            .to_string()
            .contains("[limit-exceeded]")
    );
    raw["version"] = json!(7);
    assert!(transaction::validate_value(&raw).is_err());
}

#[test]
fn cli_adjustments_use_one_guarded_history_entry_and_export_all_formats() {
    let directory = std::env::temp_dir().join(format!("pentool-v080-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("document.pen");
    std::fs::write(&file, serde_json::to_vec(&document()).unwrap()).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(args)
            .output()
            .unwrap()
    };
    let path = file.to_str().unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let dry = run(&[
        "adjustment",
        "add",
        path,
        "grade",
        "--kind",
        "invert",
        "--dry-run",
    ]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let revision = transaction::revision(&bytes);
    let output = run(&[
        "adjustment",
        "add",
        path,
        "grade",
        "--kind",
        "invert",
        "--if-revision",
        &revision,
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let committed = std::fs::read(&file).unwrap();
    assert!(!run(&[
        "adjustment",
        "disable",
        path,
        "grade",
        "--if-revision",
        &revision
    ])
    .status
    .success());
    assert_eq!(std::fs::read(&file).unwrap(), committed);
    for extension in ["png", "svg", "pdf"] {
        let destination = directory.join(format!("export.{extension}"));
        let output = run(&["export", path, destination.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{extension}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(std::fs::metadata(destination).unwrap().len() > 32);
    }
    pentool::history::undo(&file).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    pentool::history::redo(&file).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), committed);
    std::fs::remove_dir_all(directory).unwrap();
}
