//! Plate separations: deterministic plate order and knockout/overprint semantics.
use pentool::color::{
    catalog,
    separation::{self, Options, Separations},
    spot::{self, Ink, Library, Provenance},
};
use serde_json::{json, Value};
use std::path::Path;

const DOCUMENT: &str = "target/separation.pen";

fn ink(id: &str) -> Ink {
    Ink {
        id: id.into(),
        name: format!("Ink {id}"),
        aliases: vec![],
        alternate: json!({"space":"srgb","channels":[0.95,0.22,0.03]}),
        provenance: Provenance {
            source: "test library".into(),
            publisher: None,
            revision: None,
            license: None,
        },
        redistribution: "allowed".into(),
    }
}

fn cmyk(c: [f64; 4]) -> Value {
    json!({"space":"icc-cmyk","profile":"press","channels":c})
}

fn rect(id: &str, x: u32, width: u32, fill: Value, extra: Value) -> Value {
    let mut node = json!({"kind":"rect","id":id,"x":x,"y":0,"width":width,"height":2,
        "style":{"fill":fill}});
    for (key, value) in extra.as_object().into_iter().flatten() {
        node["style"][key] = value.clone();
    }
    node
}

fn doc(nodes: Vec<Value>) -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":8,"height":2,"background":"#00000000"});
    catalog::import(
        &mut raw,
        Path::new(DOCUMENT),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    raw["pages"][0]["layers"][0]["nodes"] = json!(nodes);
    raw["color_management"]["roles"] = json!({"output":{
        "destination":{"space":"icc-cmyk","profile":"press"},"intent":"relative-colorimetric"}});
    let mut library = Library::default();
    for id in ["zeta", "alpha"] {
        library.insert(ink(id)).unwrap();
    }
    raw["color_management"]["inks"] = serde_json::to_value(&library).unwrap();
    raw
}

fn separate(raw: &Value) -> anyhow::Result<Separations> {
    separation::separate(
        raw,
        Path::new(DOCUMENT),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        },
    )
}

fn at(result: &Separations, plate: &str, x: usize) -> f64 {
    let plate = result.plates.iter().find(|p| p.id == plate).unwrap();
    plate.samples[x]
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn spot_fill(id: &str, tint: f64) -> Value {
    json!({"spot":{"ink":id,"tint":tint},
        "fallback":{"space":"srgb","channels":[0.95,0.22,0.03]}})
}

#[test]
fn knockout_clears_every_underlying_plate_in_the_painted_area() {
    let raw = doc(vec![
        rect(
            "base",
            0,
            8,
            json!({"fallback":cmyk([0.8, 0.6, 0.4, 0.2])}),
            json!({}),
        ),
        rect("spot", 0, 4, spot_fill("alpha", 0.5), json!({})),
    ]);
    let before = raw.clone();
    let result = separate(&raw).unwrap();
    assert_eq!(raw, before);
    let ids: Vec<_> = result.plates.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["C", "M", "Y", "K", "alpha"]);
    // Knocked-out half: process plates cleared, spot at tint.
    for plate in ["C", "M", "Y", "K"] {
        near(at(&result, plate, 0), 0.0);
    }
    near(at(&result, "alpha", 0), 0.5);
    // Untouched half keeps the base separation.
    near(at(&result, "C", 7), 0.8);
    near(at(&result, "K", 7), 0.2);
    near(at(&result, "alpha", 7), 0.0);
    assert_eq!((result.knockouts, result.overprints), (2, 0));
}

#[test]
fn overprint_retains_plates_and_mode_controls_zero_process_channels() {
    let over = |mode: u64, fill: Value| {
        doc(vec![
            rect(
                "base",
                0,
                8,
                json!({"fallback":cmyk([0.8, 0.6, 0.4, 0.2])}),
                json!({}),
            ),
            rect(
                "top",
                0,
                4,
                fill,
                json!({"overprint":{"fill":true,"mode":mode}}),
            ),
        ])
    };
    let magenta = json!({"fallback":cmyk([0.0, 1.0, 0.0, 0.0])});
    let mode0 = separate(&over(0, magenta.clone())).unwrap();
    // Mode 0: the paint's zero process channels still replace the backdrop.
    near(at(&mode0, "C", 0), 0.0);
    near(at(&mode0, "M", 0), 1.0);
    near(at(&mode0, "K", 0), 0.0);
    let mode1 = separate(&over(1, magenta)).unwrap();
    near(at(&mode1, "C", 0), 0.8);
    near(at(&mode1, "M", 0), 1.0);
    near(at(&mode1, "Y", 0), 0.4);
    near(at(&mode1, "K", 0), 0.2);
    assert_eq!(mode1.overprints, 1);
    // A spot overprint touches its own plate only and leaves process intact.
    let spot = separate(&over(0, spot_fill("zeta", 0.25))).unwrap();
    near(at(&spot, "C", 0), 0.8);
    near(at(&spot, "K", 0), 0.2);
    near(at(&spot, "zeta", 0), 0.25);
    near(at(&spot, "zeta", 7), 0.0);
}

#[test]
fn spot_plates_follow_process_in_lexical_order_and_stay_separate() {
    let raw = doc(vec![
        rect("one", 0, 4, spot_fill("zeta", 1.0), json!({})),
        rect("two", 2, 4, spot_fill("alpha", 0.5), json!({})),
    ]);
    let result = separate(&raw).unwrap();
    let ids: Vec<_> = result.plates.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["C", "M", "Y", "K", "alpha", "zeta"]);
    near(at(&result, "zeta", 0), 1.0);
    near(at(&result, "alpha", 2), 0.5);
    // Knocking out the second ink's area also clears the first ink there.
    near(at(&result, "zeta", 2), 0.0);
    // Only spot paints: no CMYK destination is needed.
    let mut no_role = raw.clone();
    no_role["color_management"]["roles"] = json!({});
    assert!(separate(&no_role).is_ok());
    assert_eq!(result.json()["scope"], "plate-separations");
}

#[test]
fn unsupported_transparency_and_paints_fail_with_the_node_named() {
    let base = |node: Value| doc(vec![node]);
    let fill = json!({"fallback":cmyk([1.0, 0.0, 0.0, 0.0])});
    let mut translucent = rect("t", 0, 4, fill.clone(), json!({}));
    translucent["opacity"] = json!(0.5);
    let mut blended = rect("b", 0, 4, fill.clone(), json!({}));
    blended["blend_mode"] = json!("multiply");
    let isolated = json!({"kind":"group","id":"g","isolation":"isolated","children":[
        rect("inner", 0, 4, fill.clone(), json!({"overprint":{"fill":true}}))]});
    let gradient = rect(
        "gradient",
        0,
        4,
        json!({"kind":"linear-gradient","stops":[]}),
        json!({}),
    );
    let partial = rect(
        "partial",
        0,
        4,
        json!({"fallback":{"space":"icc-cmyk","profile":"press","channels":[1,0,0,0],"alpha":0.5}}),
        json!({}),
    );
    let bad_ink = rect("unknown-ink", 0, 4, spot_fill("missing", 1.0), json!({}));
    for (node, needle) in [
        (translucent, "opacity below 1"),
        (blended, "blend mode"),
        (isolated, "isolated transparency group"),
        (gradient, "gradients"),
        (partial, "partial alpha"),
        (bad_ink, "unknown ink"),
    ] {
        let raw = base(node);
        let error = separate(&raw).unwrap_err().to_string();
        assert!(error.contains(needle), "{needle}: {error}");
    }
    // Process paints need a CMYK output role.
    let mut rgb = base(rect("r", 0, 4, fill, json!({})));
    rgb["color_management"]["roles"] =
        json!({"output":{"destination":{"space":"srgb"},"intent":"perceptual"}});
    let error = separate(&rgb).unwrap_err().to_string();
    assert!(error.contains("ICC CMYK destination"), "{error}");
}

#[test]
fn plate_export_is_deterministic_and_never_overwrites() {
    let raw = doc(vec![
        rect(
            "base",
            0,
            8,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            json!({}),
        ),
        rect("spot", 4, 4, spot_fill("alpha", 1.0), json!({})),
    ]);
    let result = separate(&raw).unwrap();
    let dir = std::env::temp_dir().join(format!("pentool-plates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let report = result.write(&dir).unwrap();
    let files: Vec<_> = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["file"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        files,
        [
            "plate-00-C.png",
            "plate-01-M.png",
            "plate-02-Y.png",
            "plate-03-K.png",
            "plate-04-spot.png"
        ]
    );
    let c = image::open(dir.join(&files[0])).unwrap().to_luma8();
    assert_eq!(c.dimensions(), (8, 2));
    // 0.5 cyan ink is a mid gray negative; the knocked-out spot half is white paper.
    assert_eq!(c.get_pixel(0, 0).0[0], 128);
    assert_eq!(c.get_pixel(7, 0).0[0], 255);
    assert!(dir.join("separations.json").is_file());
    assert!(result
        .write(&dir)
        .unwrap_err()
        .to_string()
        .contains("already exists"));
    // Same input, same digests.
    let again = separate(&raw).unwrap().json();
    assert_eq!(again["plates"], report["plates"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(spot::document_library(&raw)
        .unwrap()
        .inks
        .contains_key("alpha"));
}

#[test]
fn malformed_declarations_fail_document_validation() {
    let mut raw = doc(vec![rect(
        "bad",
        0,
        4,
        spot_fill("alpha", 0.5),
        json!({"overprint":{"fill":true,"mode":2}}),
    )]);
    let error = pentool::scene::validate(&raw).unwrap_err().to_string();
    assert!(error.contains("overprint mode"), "{error}");
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["overprint"]["mode"] = json!(1);
    pentool::scene::validate(&raw).unwrap();
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"]["spot"]["ink"] = json!("missing");
    let error = pentool::scene::validate(&raw).unwrap_err().to_string();
    assert!(error.contains("unknown ink"), "{error}");
}

#[test]
fn cli_separate_writes_plates_and_refuses_existing_output() {
    let raw = doc(vec![
        rect(
            "base",
            0,
            8,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            json!({}),
        ),
        rect("spot", 4, 4, spot_fill("alpha", 1.0), json!({})),
    ]);
    let dir = std::env::temp_dir().join(format!("pentool-cli-sep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("doc.pen");
    std::fs::write(&input, serde_json::to_vec(&raw).unwrap()).unwrap();
    let before = std::fs::read(&input).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["color", "separate"])
            .arg(&input)
            .arg("--out")
            .arg(dir.join("plates"))
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["scope"], "plate-separations");
    assert_eq!(report["plates"].as_array().unwrap().len(), 5);
    assert!(dir.join("plates/plate-04-spot.png").is_file());
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let second = run();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already exists"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sixteen_bit_plates_keep_precision_the_eight_bit_plates_lose() {
    // 0.5001 is 127.5255 in 8 bits (rounds to a 128 step) but 32771.5 in 16 bits.
    let raw = doc(vec![rect(
        "base",
        0,
        8,
        json!({"fallback":cmyk([0.5001, 0.0, 0.0, 0.0])}),
        json!({}),
    )]);
    let result = separate(&raw).unwrap();
    let root = std::env::temp_dir().join(format!("pentool-plates16-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let report = result.write_with_depth(&root.join("p16"), 16).unwrap();
    assert_eq!(report["bits"], 16);
    assert_eq!(report["files"][0]["bits"], 16);
    let image = image::open(root.join("p16/plate-00-C.png")).unwrap();
    assert!(matches!(image, image::DynamicImage::ImageLuma16(_)));
    let luma = image.to_luma16();
    let want = (65535.0_f64 - at(&result, "C", 0) * 65535.0 + 0.5).floor() as u16;
    assert_eq!(luma.get_pixel(0, 0).0[0], want);
    assert_ne!(want % 257, 0, "value must not be an 8-bit replicate");
    let eight = result.write_with_depth(&root.join("p8"), 8).unwrap();
    assert_eq!(eight["bits"], 8);
    assert!(matches!(
        image::open(root.join("p8/plate-00-C.png")).unwrap(),
        image::DynamicImage::ImageLuma8(_)
    ));
    let err = result.write_with_depth(&root.join("p12"), 12).unwrap_err();
    assert!(err.to_string().contains("8 or 16"));
    assert!(!root.join("p12").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cli_separate_takes_plate_bits_from_a_recipe_and_flags_override() {
    let raw = doc(vec![rect(
        "base",
        0,
        8,
        json!({"fallback":cmyk([0.5001, 0.0, 0.0, 0.0])}),
        json!({}),
    )]);
    let dir = std::env::temp_dir().join(format!("pentool-cli-recipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("doc.pen");
    std::fs::write(&input, serde_json::to_vec(&raw).unwrap()).unwrap();
    let recipe = dir.join("recipe.json");
    std::fs::write(&recipe, r#"{"version":1,"name":"plates","plate_bits":16}"#).unwrap();
    let run = |out: &str, extra: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .args(["color", "separate"])
            .arg(&input)
            .arg("--out")
            .arg(dir.join(out))
            .arg("--recipe")
            .arg(&recipe)
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(run("from-recipe", &[])["bits"], 16);
    assert_eq!(run("from-flag", &["--bits", "8"])["bits"], 8);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gradient_fill_nodes_separate_by_interpolating_device_values() {
    let gradient = json!({"kind":"fill","id":"ramp","x":0,"y":0,"width":8,"height":2,
        "fill":{"kind":"linear","x1":0,"y1":0,"x2":8,"y2":0,"stops":[
            {"position":0,"color":cmyk([0.0, 0.0, 0.0, 0.0])},
            {"position":1,"color":cmyk([0.0, 0.0, 0.0, 1.0])}]}});
    let result = separate(&doc(vec![gradient.clone()])).unwrap();
    let k: Vec<f64> = (0..8).map(|x| at(&result, "K", x)).collect();
    assert!(k.windows(2).all(|w| w[1] >= w[0]), "{k:?}");
    assert!(k[0] < 0.2 && k[7] > 0.8, "{k:?}");
    assert!(at(&result, "C", 4) < 0.05, "no cyan on a K-only ramp");
    let again = separate(&doc(vec![gradient.clone()])).unwrap();
    assert_eq!(
        result.plates.iter().map(|p| &p.samples).collect::<Vec<_>>(),
        again.plates.iter().map(|p| &p.samples).collect::<Vec<_>>()
    );

    let mut translucent = gradient.clone();
    translucent["fill"]["stops"][1]["color"] = json!({"space":"icc-cmyk","profile":"press",
        "channels":[0.0,0.0,0.0,1.0],"alpha":0.5});
    let error = separate(&doc(vec![translucent])).unwrap_err().to_string();
    assert!(error.contains("partial alpha"), "{error}");

    let mut pattern = gradient;
    pattern["fill"]["kind"] = json!("pattern");
    assert!(separate(&doc(vec![pattern])).is_err());
}

#[test]
fn embedded_images_separate_through_the_destination_profile() {
    let mut raw = doc(vec![]);
    let mut png = Vec::new();
    // Left half red, right half white.
    let mut source = image::RgbImage::from_pixel(8, 2, image::Rgb([255, 255, 255]));
    for y in 0..2 {
        for x in 0..4 {
            source.put_pixel(x, y, image::Rgb([220, 20, 20]));
        }
    }
    source
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let layer = raw["pages"][0]["layers"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    pentool::image::add(
        &mut raw,
        None,
        &layer,
        "photo",
        &png,
        pentool::image::embedded_storage(&png),
        0.0,
        0.0,
        8.0,
        2.0,
        pentool::image::Fit::Fill,
    )
    .unwrap();
    let result = separate(&raw).unwrap();
    assert!(
        at(&result, "M", 1) > 0.5 && at(&result, "Y", 1) > 0.5,
        "red areas carry magenta and yellow"
    );
    assert!(at(&result, "C", 1) < 0.2, "red carries little cyan");
    for plate in ["C", "M", "Y", "K"] {
        assert!(
            at(&result, plate, 6) < 0.1,
            "white paper stays clear on {plate}"
        );
    }
}

fn trapped(raw: &Value, width: Option<f64>) -> Separations {
    separation::separate(
        raw,
        Path::new(DOCUMENT),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: width,
        },
    )
    .unwrap()
}

#[test]
fn automatic_trap_spreads_the_lighter_ink_under_the_darker_neighbor() {
    // Light cyan (x 0..4) abuts black (x 4..8): the cyan must creep under the
    // black edge, never the other way round, and nowhere else.
    let raw = doc(vec![
        rect(
            "light",
            0,
            4,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            json!({}),
        ),
        rect(
            "dark",
            4,
            4,
            json!({"fallback":cmyk([0.0, 0.0, 0.0, 1.0])}),
            json!({}),
        ),
    ]);
    let plain = trapped(&raw, None);
    near(at(&plain, "C", 4), 0.0);
    assert_eq!(plain.auto_trapped_pixels, 0);

    let result = trapped(&raw, Some(1.0));
    near(at(&result, "C", 4), 0.5); // cyan now under the black edge
    near(at(&result, "K", 4), 1.0); // black untouched
    near(at(&result, "C", 5), 0.0); // only one pixel wide
    near(at(&result, "C", 3), 0.5); // light object unchanged
    near(at(&result, "K", 3), 0.0); // dark ink never spreads outward
    assert_eq!(result.auto_trapped_pixels, 2); // one pixel per row

    // Deterministic.
    assert_eq!(result, trapped(&raw, Some(1.0)));
    // Invalid and over-limit widths are refused before work.
    let error = separation::separate(
        &raw,
        Path::new(DOCUMENT),
        None,
        &Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: Some(20.0),
        },
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("[limit-exceeded]"),
        "{error:#}"
    );
}

#[test]
fn automatic_trap_leaves_paper_and_soft_steps_alone() {
    // Ink beside bare paper has nothing lighter to spread; a 0.1 density step is
    // below the edge threshold.
    let raw = doc(vec![
        rect(
            "a",
            0,
            4,
            json!({"fallback":cmyk([0.0, 0.0, 0.0, 0.5])}),
            json!({}),
        ),
        rect(
            "b",
            4,
            2,
            json!({"fallback":cmyk([0.0, 0.0, 0.0, 0.6])}),
            json!({}),
        ),
    ]);
    let result = trapped(&raw, Some(2.0));
    assert_eq!(result.auto_trapped_pixels, 0);
    near(at(&result, "K", 6), 0.0);
    near(at(&result, "K", 4), 0.6);
}

#[test]
fn composite_preview_reads_plates_back_and_multiplies_spots() {
    let raw = doc(vec![
        rect(
            "ink",
            0,
            2,
            json!({"fallback":cmyk([0.0, 0.0, 0.0, 1.0])}),
            json!({}),
        ),
        rect("spot", 2, 2, spot_fill("alpha", 1.0), json!({})),
    ]);
    let result = trapped(&raw, None);
    let (png, report) = result.composite_preview(&raw, Path::new(DOCUMENT)).unwrap();
    assert_eq!(report["scope"], "composite-plate-preview");
    assert_eq!(report["spectral_paper_or_ink_model"], false);
    let image = image::load_from_memory(&png).unwrap().to_rgb8();
    assert_eq!(image.dimensions(), (8, 2));
    let black = image.get_pixel(0, 0).0;
    let paper = image.get_pixel(6, 0).0;
    let spot = image.get_pixel(3, 0).0;
    assert!(
        black.iter().all(|c| *c < 80),
        "full K reads dark: {black:?}"
    );
    assert!(
        paper.iter().all(|c| *c > 200),
        "bare paper reads light: {paper:?}"
    );
    assert_ne!(spot, paper, "the spot plate tints its alternate over paper");
    // Deterministic.
    let (again, _) = result.composite_preview(&raw, Path::new(DOCUMENT)).unwrap();
    assert_eq!(png, again);
}

#[test]
fn cli_separate_composite_and_auto_trap_flags_report_their_results() {
    let raw = doc(vec![
        rect(
            "light",
            0,
            4,
            json!({"fallback":cmyk([0.5, 0.0, 0.0, 0.0])}),
            json!({}),
        ),
        rect(
            "dark",
            4,
            4,
            json!({"fallback":cmyk([0.0, 0.0, 0.0, 1.0])}),
            json!({}),
        ),
    ]);
    let dir = std::env::temp_dir().join(format!("pentool-cli-comp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("doc.pen");
    std::fs::write(&input, serde_json::to_vec(&raw).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
        .args(["color", "separate"])
        .arg(&input)
        .arg("--out")
        .arg(dir.join("plates"))
        .args(["--composite", "--auto-trap", "1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["auto_trapped_pixels"], 2);
    assert_eq!(report["composite_preview"]["file"], "composite-preview.png");
    assert_eq!(
        report["composite_preview"]["report"]["scope"],
        "composite-plate-preview"
    );
    assert!(dir.join("plates/composite-preview.png").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn photo_nodes_separate_like_images() {
    let raw = doc(vec![]);
    let layer = raw["pages"][0]["layers"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dir = std::env::temp_dir().join(format!("pentool-photo-sep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy("docs/fixtures/photo-dng-rggb16.dng", dir.join("f.dng")).unwrap();
    std::fs::write(dir.join("doc.pen"), serde_json::to_vec(&raw).unwrap()).unwrap();
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    run(&["raw", "add", "doc.pen", "f", "--file", "f.dng"]);
    run(&[
        "photo", "place", "doc.pen", "f", "--layer", &layer, "--width", "8", "--height", "8",
    ]);
    let report = run(&["color", "separate", "doc.pen", "--out", "plates"]);
    assert_eq!(report["objects"], 1, "{report}");
    let inked: u64 = report["plates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|plate| plate["inked_pixels"].as_u64().unwrap())
        .sum();
    assert!(
        inked > 0,
        "the developed photo puts ink on the plates: {report}"
    );
    assert!(dir.join("plates/plate-01-M.png").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn raster_layers_separate_like_images() {
    let mut raw = doc(vec![]);
    pentool::raster::add(&mut raw, None, None, "paint", 0.0, 0.0, 8, 2).unwrap();
    let result = separate(&raw).expect("raster layers are separable");
    assert_eq!(result.objects, 1);
}

#[test]
fn spot_only_composite_uses_configured_or_explicit_output() {
    let raw = doc(vec![rect("spot", 2, 2, spot_fill("alpha", 1.0), json!({}))]);
    for explicit in [false, true] {
        let mut options = Options {
            destination: None,
            intent: None,
            scale: 1.0,
            auto_trap: None,
        };
        if explicit {
            options.destination = Some(
                pentool::color::value::Destination::parse(
                    &json!({"space":"icc-cmyk","profile":"press"}),
                )
                .unwrap(),
            );
        }
        let result = separation::separate(&raw, Path::new(DOCUMENT), None, &options).unwrap();
        assert!(result.plates[..4]
            .iter()
            .all(|p| p.samples.iter().all(|v| *v == 0.0)));
        let (png, _) = result.composite_preview(&raw, Path::new(DOCUMENT)).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgb8();
        let paper = image.get_pixel(6, 0).0;
        assert!(paper.iter().all(|v| *v > 200));
        assert_ne!(image.get_pixel(3, 0).0, paper);
    }
}
