//! Native channel evaluation uses the same PCS and ICC links as production paths.
use pentool::{
    color::{
        catalog,
        icc::Intent,
        pcs,
        value::{self, Destination, Evaluator, NativeColor, ProfileSet},
    },
    photo::color::NAMED_SPACES,
    resource,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

const RGB: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const CMYK: &[u8] = include_bytes!("../docs/fixtures/print-cmyk-mab-v4.icc");
fn document() -> Value {
    serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap()
}
fn sample(space: &str, channels: &[f64], profile: Option<&str>, alpha: f64) -> Value {
    let mut color = json!({"space":space,"channels":channels,"alpha":alpha});
    if let Some(profile) = profile {
        color["profile"] = json!(profile);
    }
    color
}
fn near(actual: &[f64], expected: &[f64], tolerance: f64) {
    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(expected) {
        assert!((a - b).abs() <= tolerance, "{actual:?} != {expected:?}");
    }
}

#[test]
fn native_values_require_explicit_spaces_counts_profiles_and_separate_alpha() {
    for space in NAMED_SPACES {
        for name in [space.to_owned(), format!("{space}:linear")] {
            let raw = sample(&name, &[0.1, 0.2, 0.3], None, 0.4);
            let parsed = NativeColor::parse(&raw).unwrap();
            assert_eq!(parsed.json(), raw);
            assert_eq!(parsed.destination().channel_count(), 3);
        }
    }
    for (space, channels, profile) in [
        ("icc-gray", vec![0.5], Some("gray")),
        ("icc-cmyk", vec![0.0, 0.1, 0.2, 1.0], Some("profile:press")),
        ("icc-rgb", vec![0.1, 0.2, 0.3], Some("rgb")),
        ("lab-d50", vec![120.0, -160.0, 140.0], None),
    ] {
        let raw = sample(space, &channels, profile, 0.0);
        assert_eq!(NativeColor::parse(&raw).unwrap().json(), raw);
    }
    for raw in [
        json!(null),
        json!("#fff"),
        json!({"space":"srgb","channels":[0,0,0,1]}),
        json!({"space":"srgb","channels":[0,0,-0.1]}),
        json!({"space":"icc-cmyk","channels":[0,0,0,0]}),
        json!({"space":"srgb","channels":[0,0,0],"profile":"rgb"}),
        json!({"space":"xyz","channels":[0,0,0]}),
        json!({"space":"srgb","channels":[0,0,0],"alpha":null}),
        json!({"space":"srgb","channels":[0,0,0],"alpha":1.1}),
        json!({"space":"srgb","channels":[0,0,0],"intent":"perceptual"}),
        json!({"space":"lab-d50","channels":[50,1e300,0]}),
        json!({"space":"icc-rgb","profile":"","channels":[0,0,0]}),
    ] {
        assert!(NativeColor::parse(&raw).is_err(), "accepted {raw}");
    }
    assert!(Destination::parse(&json!({"space":"srgb","channels":[0,0,0]})).is_err());
    assert_eq!(
        NativeColor::parse(&json!({"space":"srgb","channels":[0,0,0]}))
            .unwrap()
            .alpha(),
        1.0
    );
    assert!("relative".parse::<Intent>().is_err());
}

#[test]
fn named_space_white_is_exact_physical_icc_d50_and_round_trips_retain_alpha() {
    let set = ProfileSet::default();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    for name in NAMED_SPACES {
        let white = NativeColor::parse(&sample(name, &[1.0; 3], None, 0.25)).unwrap();
        near(&engine.to_xyz(&white).unwrap().0, &pcs::D50, 1e-12);
        let white_rgb = engine
            .convert(
                &white,
                &Destination::parse(&json!({"space":"srgb"})).unwrap(),
            )
            .unwrap();
        assert!(
            !white_rgb.clipped,
            "{name} white incorrectly reported as outside RGB gamut"
        );
        near(white_rgb.color.channels(), &[1.0; 3], 1e-12);
        let source =
            NativeColor::parse(&sample(name, &[0.21, 0.43, 0.67], None, 0.123456)).unwrap();
        let lab = engine
            .convert(
                &source,
                &Destination::parse(&json!({"space":"lab-d50"})).unwrap(),
            )
            .unwrap();
        let back = engine.convert(&lab.color, source.destination()).unwrap();
        near(back.color.channels(), source.channels(), 1e-11);
        assert_eq!(back.color.alpha(), source.alpha());
        assert!(!back.clipped);
        let identity = engine.convert(&source, source.destination()).unwrap();
        assert_eq!(identity.color, source);
        assert_eq!(identity.path, "identity");
    }
    let lab = NativeColor::parse(&sample("lab-d50", &[100.0, 0.0, 0.0], None, 0.0)).unwrap();
    near(&engine.to_xyz(&lab).unwrap().0, &pcs::D50, 1e-12);
    let out = engine
        .convert(
            &NativeColor::parse(&sample("display-p3", &[1.0, 0.0, 0.0], None, 0.5)).unwrap(),
            &Destination::parse(&json!({"space":"srgb"})).unwrap(),
        )
        .unwrap();
    assert!(out.clipped);
    near(out.color.channels(), &[1.0, 0.0, 0.0], 1e-12);
    assert_eq!(out.color.alpha(), 0.5);
    let linear = engine
        .convert(
            &NativeColor::parse(&sample("srgb", &[0.5; 3], None, 1.0)).unwrap(),
            &Destination::parse(&json!({"space":"srgb:linear"})).unwrap(),
        )
        .unwrap();
    near(linear.color.channels(), &[0.21404114048223255; 3], 1e-12);
}

#[test]
fn native_icc_profile_channels_and_missing_intent_paths_are_checked_without_mutation() {
    let path = Path::new("target/native-color.pen");
    let mut raw = document();
    catalog::import(&mut raw, path, RGB, "rgb", "allowed", None, None).unwrap();
    catalog::import(&mut raw, path, CMYK, "cmyk", "allowed", None, None).unwrap();
    let before = raw.clone();
    let source = sample("icc-rgb", &[0.2, 0.4, 0.7], Some("rgb"), 0.0);
    let result = value::evaluate(
        &raw,
        path,
        &source,
        Some(&json!({"space":"lab-d50"})),
        Intent::RelativeColorimetric,
    )
    .unwrap();
    assert_eq!(result["source"], source);
    assert_eq!(result["converted"]["color"]["alpha"], 0.0);
    assert_eq!(result["mutated"], false);
    for bad in [
        sample("icc-cmyk", &[0.0; 4], Some("rgb"), 1.0),
        sample("icc-rgb", &[0.0; 3], Some("cmyk"), 1.0),
        sample("icc-rgb", &[0.0; 3], Some("missing"), 1.0),
    ] {
        assert!(value::evaluate(&raw, path, &bad, None, Intent::RelativeColorimetric).is_err());
    }
    assert!(value::evaluate(
        &raw,
        path,
        &source,
        Some(&json!({"space":"icc-cmyk","profile":"cmyk"})),
        Intent::RelativeColorimetric
    )
    .is_err());
    let result = value::evaluate(
        &raw,
        path,
        &sample("icc-cmyk", &[0.1, 0.2, 0.3, 0.4], Some("cmyk"), 0.0),
        Some(&json!({"space":"icc-rgb","profile":"rgb"})),
        Intent::Perceptual,
    )
    .unwrap();
    assert_eq!(
        result["converted"]["color"]["channels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(raw, before);
    assert!(value::evaluate(
        &catalog::downgrade(document()).unwrap(),
        path,
        &sample("srgb", &[0.0; 3], None, 1.0),
        None,
        Intent::RelativeColorimetric
    )
    .is_err());
}

#[test]
fn native_icc_conversion_matches_independent_all_intent_vectors() {
    let path = Path::new("target/native-vectors.pen");
    let mut raw = document();
    let press = std::fs::read("docs/fixtures/print-CGATS21_CRPC7.icc").unwrap();
    catalog::import(&mut raw, path, RGB, "rgb", "allowed", None, None).unwrap();
    catalog::import(&mut raw, path, &press, "press", "allowed", None, None).unwrap();
    let vectors: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-press-vectors.json"))
            .unwrap();
    let set = ProfileSet::load(&raw, path, ["rgb", "press"]).unwrap();
    for (number, intent) in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ]
    .into_iter()
    .enumerate()
    {
        let mut engine = Evaluator::new(&set, intent).unwrap();
        for (field, space, profile, to_space, to_profile) in [
            ("rgb_to_cmyk_vectors", "icc-rgb", "rgb", "icc-cmyk", "press"),
            ("cmyk_to_rgb_vectors", "icc-cmyk", "press", "icc-rgb", "rgb"),
        ] {
            let destination =
                Destination::parse(&json!({"space":to_space,"profile":to_profile})).unwrap();
            for vector in vectors[field]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v["intent"] == number)
            {
                let input: Vec<f64> = vector["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let source =
                    NativeColor::parse(&sample(space, &input, Some(profile), 0.37)).unwrap();
                let result = engine.convert(&source, &destination).unwrap();
                let expected: Vec<f64> = vector["output"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                near(
                    result.color.channels(),
                    &expected,
                    vector["tolerance_channel"].as_f64().unwrap(),
                );
                assert_eq!(result.color.alpha(), 0.37);
                assert_eq!(result.link.unwrap().intent, intent);
            }
        }
    }
    assert_eq!(
        resource::sha256(&press),
        format!("sha256:{}", vectors["sha256"].as_str().unwrap())
    );
}

#[test]
fn gray_channels_and_alias_identity_use_the_declared_profile_without_reseparation() {
    let path = Path::new("target/native-gray.pen");
    let mut raw = document();
    let mut gray = RGB.to_vec();
    gray[16..20].copy_from_slice(b"GRAY");
    let tags = u32::from_be_bytes(gray[128..132].try_into().unwrap()) as usize;
    for i in 0..tags {
        let start = 132 + i * 12;
        if &gray[start..start + 4] == b"rTRC" {
            gray[start..start + 4].copy_from_slice(b"kTRC");
        }
    }
    catalog::import(&mut raw, path, &gray, "gray", "allowed", None, None).unwrap();
    catalog::import(&mut raw, path, &gray, "second-gray", "allowed", None, None).unwrap();
    let set = ProfileSet::load(&raw, path, ["gray", "second-gray"]).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let source = NativeColor::parse(&sample("icc-gray", &[0.4], Some("gray"), 0.2)).unwrap();
    near(
        &engine.to_xyz(&source).unwrap().0,
        &pcs::D50.map(|v| v * 0.4),
        1e-12,
    );
    let identity = engine
        .convert(
            &source,
            &Destination::parse(&json!({"space":"icc-gray","profile":"second-gray"})).unwrap(),
        )
        .unwrap();
    assert_eq!(identity.color.channels(), source.channels());
    assert_eq!(identity.color.alpha(), 0.2);
    assert_eq!(identity.path, "identity");
    assert!(identity.link.is_none());
    let lab = engine
        .convert(
            &source,
            &Destination::parse(&json!({"space":"lab-d50"})).unwrap(),
        )
        .unwrap();
    let back = engine.convert(&lab.color, source.destination()).unwrap();
    near(back.color.channels(), source.channels(), 1e-12);
    let mut device_link = RGB.to_vec();
    device_link[12..16].copy_from_slice(b"link");
    device_link[20..24].copy_from_slice(b"RGB ");
    catalog::import(&mut raw, path, &device_link, "link", "allowed", None, None).unwrap();
    let error = value::evaluate(
        &raw,
        path,
        &sample("icc-rgb", &[0.1; 3], Some("link"), 1.0),
        None,
        Intent::Perceptual,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("explicit device-link workflow"));
}

#[test]
fn compilation_cache_reuses_selected_pairs_and_has_an_explicit_bound() {
    let path = Path::new("target/native-cache.pen");
    let mut raw = document();
    let names: Vec<String> = (0..9).map(|i| format!("rgb-{i}")).collect();
    for (i, name) in names.iter().enumerate() {
        let mut bytes = RGB.to_vec();
        bytes[48..52].copy_from_slice(&[b'T', b'E', b'S', b'0' + i as u8]);
        catalog::import(&mut raw, path, &bytes, name, "allowed", None, None).unwrap();
    }
    let set = ProfileSet::load(&raw, path, names.iter().map(String::as_str)).unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let mut compiled = 0;
    for source in &names {
        for destination in &names {
            if source == destination {
                continue;
            }
            let source =
                NativeColor::parse(&sample("icc-rgb", &[0.2, 0.4, 0.6], Some(source), 0.8))
                    .unwrap();
            let destination =
                Destination::parse(&json!({"space":"icc-rgb","profile":destination})).unwrap();
            if compiled < value::MAX_COMPILED_LINKS {
                let result = engine.convert(&source, &destination).unwrap();
                near(result.color.channels(), source.channels(), 1e-11);
                // Re-evaluating the same pair must not consume another cache slot.
                engine.convert(&source, &destination).unwrap();
                compiled += 1;
            } else {
                assert!(
                    format!("{:#}", engine.convert(&source, &destination).unwrap_err())
                        .contains("compiled ICC links")
                );
            }
        }
    }
    assert_eq!(compiled, value::MAX_COMPILED_LINKS);
}

#[test]
fn cli_sample_evaluation_is_read_only_and_reports_native_channels_and_clipping() {
    let root = std::env::temp_dir().join(format!(
        "pentool-native-color-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("art.pen");
    let before = serde_json::to_vec(&document()).unwrap();
    std::fs::write(&path, &before).unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_pentool"))
            .current_dir(&root)
            .args(["color", "evaluate", "art.pen"])
            .args(extra)
            .output()
            .unwrap()
    };
    let out = run(&[
        "--color",
        r#"{"space":"display-p3","channels":[1,0,0],"alpha":0.4}"#,
        "--to",
        r#"{"space":"srgb"}"#,
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["converted"]["clipped"], true);
    assert_eq!(result["converted"]["color"]["alpha"], 0.4);
    assert!(!run(&[
        "--color",
        r#"{"space":"icc-cmyk","channels":[0,0,0,0],"profile":"missing"}"#
    ])
    .status
    .success());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!root.join(".pentool").exists());
    std::fs::remove_dir_all(root).unwrap();
}
