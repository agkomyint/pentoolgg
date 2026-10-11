//! Native interpolation precedes appearance conversion in every fill path.
use pentool::{
    color::{
        catalog,
        icc::Intent,
        scene::{destinations, Scope},
        value,
    },
    composite, resource, scene,
};
use serde_json::{json, Value};
use std::path::Path;
const PATH: &str = "target/native-gradient.pen";
fn doc() -> Value {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    raw["pages"][0]["canvas"] = json!({"width":4,"height":1,"background":"#00000000"});
    raw["pages"][0]["layers"][0]["nodes"] = json!([{"kind":"fill","id":"ramp","x":0,"y":0,"width":4,"height":1,"fill":{"kind":"linear","space":{"space":"srgb:linear"},"intent":"relative-colorimetric","stops":[{"position":0,"color":"#000"},{"position":1,"color":"#fff"}]}}]);
    raw
}
fn fill(raw: &mut Value) -> &mut Value {
    &mut raw["pages"][0]["layers"][0]["nodes"][0]["fill"]
}
fn render(raw: &Value) -> image::RgbaImage {
    composite::render(raw, Path::new(PATH), None, 1.0).unwrap()
}
fn color(space: &str, channels: &[f64], alpha: f64) -> Value {
    json!({"space":space,"channels":channels,"alpha":alpha})
}
fn transfer(v: f64) -> f64 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn byte(v: f64) -> u8 {
    (v * 255.0 + 0.5).floor() as u8
}

#[test]
fn explicit_linear_rgb_matches_independent_pixel_math_and_legacy_valid_gradients() {
    let mut raw = doc();
    let before = raw.clone();
    let pixels = render(&raw);
    for x in 0..4 {
        let expected = byte(
            (transfer((f64::from(x) + 0.25) / 4.0) + transfer((f64::from(x) + 0.75) / 4.0)) / 2.0,
        );
        assert_eq!(
            pixels.get_pixel(x, 0).0,
            [expected, expected, expected, 255]
        );
    }
    assert_eq!(raw, before);
    fill(&mut raw)["space"] = json!("linear");
    fill(&mut raw).as_object_mut().unwrap().remove("intent");
    assert_eq!(render(&raw), pixels);
    fill(&mut raw)["space"] = json!("srgb");
    let encoded = render(&raw);
    assert_ne!(encoded, pixels);
    fill(&mut raw)["space"] = json!({"space":"srgb"});
    fill(&mut raw)["intent"] = json!("relative-colorimetric");
    assert_eq!(render(&raw), encoded);
    assert_eq!(
        composite::node_pixels(&before, Path::new(PATH), None, "ramp", 1.0).unwrap(),
        pixels
    );
}

#[test]
fn lab_and_four_channel_cmyk_interpolate_before_rgb_appearance_conversion() {
    let mut lab = doc();
    lab["pages"][0]["canvas"]["width"] = json!(1);
    lab["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(1);
    fill(&mut lab)["space"] = json!({"space":"lab-d50"});
    fill(&mut lab)["stops"][0]["color"] = color("lab-d50", &[100.0, 0.0, 0.0], 1.0);
    fill(&mut lab)["stops"][1]["color"] = color("lab-d50", &[0.0, 0.0, 0.0], 1.0);
    let expected = byte(
        (transfer(((25.0_f64 + 16.0) / 116.0).powi(3))
            + transfer(((75.0_f64 + 16.0) / 116.0).powi(3)))
            / 2.0,
    );
    assert_eq!(
        render(&lab).get_pixel(0, 0).0,
        [expected, expected, expected, 255]
    );
    let mut cmyk = lab.clone();
    catalog::import(
        &mut cmyk,
        Path::new(PATH),
        include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc"),
        "probe",
        "allowed",
        None,
        None,
    )
    .unwrap();
    fill(&mut cmyk)["space"] = json!({"space":"icc-cmyk","profile":"probe"});
    fill(&mut cmyk)["intent"] = json!("perceptual");
    for (index, value) in [0.0, 1.0].into_iter().enumerate() {
        let mut sample = color("icc-cmyk", &[value; 4], 1.0);
        sample["profile"] = json!("probe");
        fill(&mut cmyk)["stops"][index]["color"] = sample;
    }
    let native = render(&cmyk);
    for channel in &native.get_pixel(0, 0).0[..3] {
        assert!((i32::from(*channel) - i32::from(expected)).abs() <= 1);
    }
    let before = cmyk.clone();
    fill(&mut cmyk)["space"] = json!({"space":"srgb"});
    assert_ne!(render(&cmyk), native);
    assert_eq!(catalog::package_table(&before).unwrap().len(), 1);
}

#[test]
fn alpha_is_straight_and_independent_before_existing_coverage_compositing() {
    let mut raw = doc();
    raw["pages"][0]["canvas"]["width"] = json!(1);
    raw["pages"][0]["layers"][0]["nodes"][0]["width"] = json!(1);
    fill(&mut raw)["space"] = json!({"space":"srgb"});
    fill(&mut raw)["stops"][0]["color"] = color("srgb", &[1.0, 0.0, 0.0], 0.0);
    fill(&mut raw)["stops"][1]["color"] = color("srgb", &[0.0, 0.0, 1.0], 1.0);
    assert_eq!(render(&raw).get_pixel(0, 0).0, [96, 0, 159, 128]);
    fill(&mut raw)["space"] = json!("srgb");
    fill(&mut raw).as_object_mut().unwrap().remove("intent");
    assert_eq!(render(&raw).get_pixel(0, 0).0, [0, 0, 255, 128]);
}

#[test]
fn native_stops_keep_live_token_meaning_after_normal_preview_lowering() {
    let mut raw = doc();
    raw["styles"]["black"] = json!({"type":"color","value":color("lab-d50",&[0.0,0.0,0.0],1.0)});
    raw["styles"]["white"] = json!({"type":"color","value":color("lab-d50",&[100.0,0.0,0.0],1.0)});
    fill(&mut raw)["space"] = json!({"space":"lab-d50"});
    fill(&mut raw)["stops"][0]["color"] = json!({"ref":"black","fallback":"#f00"});
    fill(&mut raw)["stops"][1]["color"] = json!({"ref":"white","fallback":"#00f"});
    let before = raw.clone();
    let actual = render(&raw);
    let mut direct = raw.clone();
    fill(&mut direct)["stops"][0]["color"] = raw["styles"]["black"]["value"].clone();
    fill(&mut direct)["stops"][1]["color"] = raw["styles"]["white"]["value"].clone();
    assert_eq!(actual, render(&direct));
    assert_eq!(raw, before);
    let definition = fill(&mut raw).clone();
    raw["styles"]["ramp-style"] = json!({"type":"fill","value":definition});
    *fill(&mut raw) = json!({"ref":"ramp-style","fallback":{"kind":"solid","color":"#f00"}});
    assert_eq!(actual, render(&raw));
}

#[test]
fn interpolation_destination_references_block_removal_and_unrepresentable_downgrade() {
    let mut raw = doc();
    catalog::import(
        &mut raw,
        Path::new(PATH),
        include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc"),
        "rgb",
        "allowed",
        None,
        None,
    )
    .unwrap();
    fill(&mut raw)["space"] = json!({"space":"icc-rgb","profile":"rgb"});
    let before = raw.clone();
    let slots = destinations(&raw, Scope::Document).unwrap();
    assert_eq!(slots.len(), 1);
    assert!(format!("{:#}", catalog::remove(&mut raw, "rgb").unwrap_err()).contains("gradient"));
    assert_eq!(raw, before);
    let mut raw = doc();
    fill(&mut raw)["space"] = json!({"space":"lab-d50"});
    assert!(format!("{:#}", catalog::downgrade(raw).unwrap_err()).contains("native gradient"));
}

#[test]
fn invalid_intents_destinations_and_unexecutable_paths_fail_without_mutation() {
    let raw = doc();
    for space in [json!(null), json!([]), json!(3), json!(true)] {
        let mut bad = raw.clone();
        fill(&mut bad)["space"] = space;
        fill(&mut bad).as_object_mut().unwrap().remove("intent");
        assert!(scene::validate(&bad).is_err());
    }
    for (space, intent) in [
        (json!({"space":"srgb"}), json!(null)),
        (
            json!({"space":"srgb","channels":[0,0,0]}),
            json!("perceptual"),
        ),
        (
            json!({"space":"icc-rgb","profile":"absent"}),
            json!("perceptual"),
        ),
        (json!({"space":"srgb"}), json!("relative")),
    ] {
        let mut bad = raw.clone();
        fill(&mut bad)["space"] = space;
        fill(&mut bad)["intent"] = intent;
        assert!(scene::validate(&bad).is_err());
    }
    let mut old = raw.clone();
    old["version"] = json!(7);
    old.as_object_mut().unwrap().remove("color_management");
    assert!(scene::validate(&old).is_err());
    let mut bad = raw.clone();
    catalog::import(
        &mut bad,
        Path::new(PATH),
        include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc"),
        "one-way",
        "allowed",
        None,
        None,
    )
    .unwrap();
    fill(&mut bad)["space"] = json!({"space":"icc-cmyk","profile":"one-way"});
    fill(&mut bad)["intent"] = json!("perceptual");
    scene::validate(&bad).unwrap();
    let before = bad.clone();
    assert!(composite::render(&bad, Path::new(PATH), None, 1.0).is_err());
    assert!(catalog::package_table(&bad).is_err());
    assert_eq!(bad, before);
}

#[test]
fn explicit_node_scope_does_not_load_other_visible_gradient_profiles() {
    let mut raw = doc();
    let rgb = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
    catalog::import(&mut raw, Path::new(PATH), rgb, "rgb", "allowed", None, None).unwrap();
    let identity = resource::sha256(rgb);
    raw["color_management"]["profiles"][&identity]["storage"] =
        json!({"kind":"external","path":"missing-native-gradient.icc"});
    let mut unrelated = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    unrelated["id"] = json!("other");
    unrelated["fill"]["space"] = json!({"space":"icc-rgb","profile":"rgb"});
    raw["pages"][0]["layers"][0]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(unrelated);
    scene::validate(&raw).unwrap();
    let before = raw.clone();
    let pixels = composite::node_pixels(&raw, Path::new(PATH), None, "ramp", 1.0).unwrap();
    assert_eq!(pixels, render(&doc()));
    assert!(composite::render(&raw, Path::new(PATH), None, 1.0).is_err());
    assert!(composite::node_pixels(&raw, Path::new(PATH), None, "missing", 1.0).is_err());
    raw["pages"][0]["layers"][0]["nodes"][1]["visible"] = json!(false);
    assert_eq!(render(&raw), pixels);
    assert_eq!(
        before["pages"][0]["layers"][0]["nodes"][1]["visible"],
        Value::Null
    );
}

#[test]
fn native_vector_mask_analysis_keeps_alias_and_digest_queries_equivalent() {
    let mut raw: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8-native.pen")).unwrap();
    raw["styles"]["ink"] = json!({"type":"color","value":color("lab-d50",&[100.0,0.0,0.0],0.5)});
    raw["pages"][0]["layers"][0]["nodes"][0]["style"]["fill"] =
        json!({"ref":"ink","fallback":"#000"});
    pentool::mask::create(&mut raw, Path::new(PATH), None, "clip", "vector:color-card").unwrap();
    let identity = raw["masks"]["clip"].as_str().unwrap();
    let before = raw.clone();
    let alias = composite::mask_coverage(&raw, Path::new(PATH), None, "clip").unwrap();
    assert_eq!(
        alias,
        composite::mask_coverage(&raw, Path::new(PATH), None, identity).unwrap()
    );
    assert_eq!(raw, before);
}

#[test]
fn real_press_gradients_sample_four_channels_with_each_explicit_intent() {
    use value::{Destination, Evaluator, NativeColor, ProfileSet};
    let mut raw = doc();
    catalog::import(
        &mut raw,
        Path::new(PATH),
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let ends = [[0.8, 0.1, 0.0, 0.2], [0.0, 0.7, 0.9, 0.8]];
    fill(&mut raw)["space"] = json!({"space":"icc-cmyk","profile":"press"});
    for (index, channels) in ends.iter().enumerate() {
        let mut sample = color("icc-cmyk", channels, 1.0);
        sample["profile"] = json!("press");
        fill(&mut raw)["stops"][index]["color"] = sample;
    }
    let set = ProfileSet::load(&raw, Path::new(PATH), ["press"]).unwrap();
    let target = Destination::parse(&json!({"space":"srgb"})).unwrap();
    for intent in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ] {
        fill(&mut raw)["intent"] = json!(intent);
        let actual = render(&raw);
        let mut engine = Evaluator::new(&set, intent).unwrap();
        for x in 0..4 {
            let mut expected = [0.0; 3];
            for sub in [0.25, 0.75] {
                let t = (f64::from(x) + sub) / 4.0;
                let channels: [f64; 4] =
                    std::array::from_fn(|i| ends[0][i] * (1.0 - t) + ends[1][i] * t);
                let mut source = color("icc-cmyk", &channels, 1.0);
                source["profile"] = json!("press");
                let converted = engine
                    .convert(&NativeColor::parse(&source).unwrap(), &target)
                    .unwrap();
                for (out, value) in expected.iter_mut().zip(converted.color.channels()) {
                    *out += value * 0.5;
                }
            }
            assert_eq!(
                actual.get_pixel(x, 0).0,
                [byte(expected[0]), byte(expected[1]), byte(expected[2]), 255],
                "{intent:?} pixel {x}"
            );
        }
        let mut rgb = raw.clone();
        fill(&mut rgb)["space"] = json!({"space":"srgb"});
        assert_ne!(
            actual,
            render(&rgb),
            "{intent:?} must not interpolate RGB endpoints"
        );
    }
}

#[test]
fn spread_geometry_and_gradient_overlay_share_native_sampling() {
    let mut raw = doc();
    fill(&mut raw)["space"] = json!({"space":"srgb"});
    fill(&mut raw)["x2"] = json!(3);
    for (spread, expected) in [
        ("pad", [43, 128, 213, 255]),
        ("repeat", [43, 128, 213, 43]),
        ("reflect", [43, 128, 213, 213]),
    ] {
        fill(&mut raw)["spread"] = json!(spread);
        let pixels = render(&raw);
        for (x, value) in expected.into_iter().enumerate() {
            // These theoretical values are exactly at half-byte ties. The
            // four coverage accumulations can differ from the closed-form
            // average by one floating-point ulp; compare legacy pixels exactly
            // below, and allow only that one-byte quantization boundary here.
            let pixel = pixels.get_pixel(x as u32, 0).0;
            for channel in &pixel[..3] {
                assert!(
                    (i32::from(*channel) - value).abs() <= 1,
                    "{spread} {x}: {pixel:?}"
                );
            }
            assert_eq!(pixel[3], 255);
        }
    }
    raw["pages"][0]["canvas"]["height"] = json!(4);
    raw["pages"][0]["layers"][0]["nodes"][0]["height"] = json!(4);
    fill(&mut raw).as_object_mut().unwrap().remove("x2");
    for kind in ["linear", "radial", "conic"] {
        for spread in ["pad", "repeat", "reflect"] {
            fill(&mut raw)["kind"] = json!(kind);
            fill(&mut raw)["spread"] = json!(spread);
            let pixels = render(&raw);
            let mut legacy = raw.clone();
            fill(&mut legacy)["space"] = json!("srgb");
            fill(&mut legacy).as_object_mut().unwrap().remove("intent");
            assert_eq!(pixels, render(&legacy), "{kind} {spread}");
        }
    }
    let direct = doc();
    let definition = direct["pages"][0]["layers"][0]["nodes"][0]["fill"].clone();
    let mut effect = direct.clone();
    effect["pages"][0]["layers"][0]["nodes"][0] = json!({"kind":"rect","id":"surface","x":0,"y":0,"width":4,"height":1,"style":{"fill":{"fallback":"#fff"}},"effects":[{"id":"grade","kind":"gradient-overlay","enabled":true,"opacity":1,"blend_mode":"normal","params":{"fill":definition}}]});
    assert_eq!(render(&effect), render(&direct));
    let stack = effect["pages"][0]["layers"][0]["nodes"][0]["effects"].clone();
    effect["styles"]["grade"] = json!({"type":"effects","value":stack});
    effect["pages"][0]["layers"][0]["nodes"][0]["effects"] = json!({"ref":"grade","fallback":[]});
    assert_eq!(render(&effect), render(&direct));
}

#[test]
fn total_native_stop_work_is_bounded_even_when_all_endpoints_are_transparent_legacy_colors() {
    let mut raw = doc();
    let mut template = raw["pages"][0]["layers"][0]["nodes"][0].clone();
    template["fill"]["stops"] = Value::Array(
        (0..64)
            .map(|i| json!({"position":f64::from(i)/63.0,"color":"none"}))
            .collect(),
    );
    raw["pages"][0]["layers"][0]["nodes"] = Value::Array(
        (0..1025)
            .map(|i| {
                let mut node = template.clone();
                node["id"] = json!(format!("ramp-{i}"));
                node
            })
            .collect(),
    );
    assert!(
        format!("{:#}", destinations(&raw, Scope::Document).unwrap_err()).contains("total stops")
    );
}

#[test]
fn existing_fill_editor_preserves_native_contract_and_rejects_locks_and_bad_intents_atomically() {
    let raw = doc();
    let mut changed = raw.clone();
    let mut definition = fill(&mut changed).clone();
    definition["space"] = json!({"space":"lab-d50"});
    pentool::fill::edit(
        &mut changed,
        None,
        "set",
        "ramp",
        &json!({"fill":definition}),
    )
    .unwrap();
    assert_eq!(fill(&mut changed)["space"], json!({"space":"lab-d50"}));
    let before = changed.clone();
    definition["intent"] = json!("unsupported");
    assert!(pentool::fill::edit(
        &mut changed,
        None,
        "set",
        "ramp",
        &json!({"fill":definition})
    )
    .is_err());
    assert_eq!(changed, before);
    changed["pages"][0]["layers"][0]["locked"] = json!(true);
    let before = changed.clone();
    assert!(pentool::fill::edit(
        &mut changed,
        None,
        "set",
        "ramp",
        &json!({"fill":raw["pages"][0]["layers"][0]["nodes"][0]["fill"]})
    )
    .is_err());
    assert_eq!(changed, before);
}

#[test]
fn normative_gradient_fixture_pins_all_pixels_and_exports_without_changing_native_samples() {
    let bytes = include_bytes!("../docs/fixtures/print-v8-gradient.pen");
    let raw: Value = serde_json::from_slice(bytes).unwrap();
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    let entry = manifest["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file"] == "print-v8-gradient.pen")
        .unwrap();
    assert_eq!(
        resource::sha256(bytes),
        format!("sha256:{}", entry["sha256"].as_str().unwrap())
    );
    scene::validate(&raw).unwrap();
    let before = raw.clone();
    let pixels = render(&raw);
    assert_eq!(pixels.dimensions(), (64, 48));
    let preview = &manifest["previews"][0];
    assert_eq!(preview["document"], "print-v8-gradient.pen");
    assert_eq!(
        resource::sha256(pixels.as_raw()),
        format!("sha256:{}", preview["sha256"].as_str().unwrap())
    );
    for x in 0..64 {
        let rgb = byte(
            (transfer((f64::from(x) + 0.25) / 64.0) + transfer((f64::from(x) + 0.75) / 64.0)) / 2.0,
        );
        assert_eq!(pixels.get_pixel(x, 8).0, [rgb, rgb, rgb, 255]);
        let gray = |sub: f64| {
            let l = (f64::from(x) + sub) * 100.0 / 64.0;
            transfer(if l > 8.0 {
                ((l + 16.0) / 116.0).powi(3)
            } else {
                l / 903.2962962962963
            })
        };
        let lab = byte((gray(0.25) + gray(0.75)) / 2.0);
        assert_eq!(pixels.get_pixel(x, 24).0, [lab, lab, lab, 255]);
        for channel in &pixels.get_pixel(x, 40).0[..3] {
            assert!((i32::from(*channel) - i32::from(lab)).abs() <= 1);
        }
    }
    assert_eq!(raw, before);
    let root = std::env::temp_dir().join(format!(
        "pentool-gradient-exports-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("ramps.pen");
    std::fs::write(&path, bytes).unwrap();
    for extension in ["png", "svg", "pdf"] {
        let output = root.join(format!("ramps.{extension}"));
        let result = std::process::Command::new(env!("CARGO_BIN_EXE_pentool"))
            .arg("export")
            .arg(&path)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(std::fs::metadata(output).unwrap().len() > 0);
    }
    assert_eq!(
        image::open(root.join("ramps.png")).unwrap().into_rgba8(),
        pixels
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(!root.join(".pentool/history").exists());
    std::fs::remove_dir_all(root).unwrap();
}
