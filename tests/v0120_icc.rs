//! Real byte-level ICC parsing and transform probes for print engine 1.
use pentool::color::icc::{MatrixTransform, Profile, MAX_PROFILE_BYTES};
use pentool::photo::{color::ColorSpace, icc};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn generated() -> Vec<u8> {
    icc::profile(&ColorSpace::named("srgb").unwrap(), 1).unwrap()
}

fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

fn fails(bytes: &[u8], code: &str) {
    let error = Profile::parse(bytes).unwrap_err().to_string();
    assert!(error.contains(code), "expected {code}: {error}");
}

#[test]
fn authored_fixtures_match_hashes_and_pcs_vectors() {
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    for fixture in manifest["fixtures"].as_array().unwrap() {
        let bytes = std::fs::read(
            std::path::Path::new("docs/fixtures").join(fixture["file"].as_str().unwrap()),
        )
        .unwrap();
        assert_eq!(fixture["sha256"], hex::encode(Sha256::digest(&bytes)));
        let profile = Profile::parse(&bytes).unwrap();
        if fixture["executor"] == "press" {
            continue;
        } // separate independent reference test below
        if fixture["executor"] == "lut" {
            let tag: &[u8; 4] = fixture["tag"]
                .as_str()
                .unwrap()
                .as_bytes()
                .try_into()
                .unwrap();
            let transform = pentool::color::icc::LutTransform::compile(&profile, tag).unwrap();
            for vector in fixture["vectors"].as_array().unwrap() {
                let device: Vec<_> = vector["device"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let (actual, _) = transform.to_xyz(&device).unwrap();
                for (i, value) in actual.iter().enumerate() {
                    assert!(
                        (value - vector["xyz"][i].as_f64().unwrap()).abs()
                            <= vector["tolerance"].as_f64().unwrap()
                    );
                }
            }
            continue;
        }
        let transform = MatrixTransform::compile(&profile).unwrap();
        for vector in fixture["vectors"].as_array().unwrap() {
            let device = std::array::from_fn(|i| vector["device"][i].as_f64().unwrap());
            let actual = transform.to_xyz(device).unwrap();
            for (i, value) in actual.iter().enumerate() {
                assert!(
                    (value - vector["xyz"][i].as_f64().unwrap()).abs()
                        <= vector["tolerance"].as_f64().unwrap()
                );
            }
            let (back, _) = transform.from_xyz(actual).unwrap();
            for i in 0..3 {
                assert!((back[i] - device[i]).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn real_press_profile_matches_independent_forward_and_reverse_intent_vectors() {
    use pentool::color::{
        icc::{Intent, Link},
        pcs,
    };
    let manifest: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-conformance.json")).unwrap();
    let bytes = std::fs::read("docs/fixtures/print-press-vectors.json").unwrap();
    assert_eq!(
        manifest["references"][0]["sha256"],
        hex::encode(Sha256::digest(&bytes))
    );
    let reference: Value = serde_json::from_slice(&bytes).unwrap();
    let bytes = std::fs::read(
        std::path::Path::new("docs/fixtures").join(reference["file"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(reference["sha256"], hex::encode(Sha256::digest(&bytes)));
    let parsed = Profile::parse(&bytes).unwrap();
    let intents = [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ];
    for vector in reference["pipeline_vectors"].as_array().unwrap() {
        let tag: &[u8; 4] = vector["tag"]
            .as_str()
            .unwrap()
            .as_bytes()
            .try_into()
            .unwrap();
        let engine = pentool::color::icc::LutTransform::compile(&parsed, tag).unwrap();
        let input: Vec<f64> = vector["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let actual = engine.evaluate(&input).unwrap();
        for (i, value) in actual.channels().iter().enumerate() {
            assert!(
                (value - vector["output"][i].as_f64().unwrap()).abs()
                    <= vector["tolerance_channel"].as_f64().unwrap(),
                "pipeline {tag:?} input {input:?}: {:?} expected {}",
                actual.channels(),
                vector["output"]
            );
        }
    }
    let mut failures = Vec::new();
    for vector in reference["vectors"].as_array().unwrap() {
        let index = vector["intent"].as_u64().unwrap() as usize;
        let intent = intents[index];
        let engine = Link::to_pcs(&parsed, intent).unwrap();
        let device: Vec<f64> = vector["device"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let sample = engine.evaluate(&device).unwrap();
        let xyz: [f64; 3] = sample.channels().try_into().unwrap();
        let lab = pcs::xyz_to_lab(xyz).unwrap();
        let delta = lab
            .iter()
            .enumerate()
            .map(|(i, v)| (v - vector["lab"][i].as_f64().unwrap()).powi(2))
            .sum::<f64>()
            .sqrt();
        if delta > vector["tolerance_delta_e76"].as_f64().unwrap() {
            failures.push(format!(
                "{intent:?}, {device:?}: Lab {lab:?}, delta={delta}"
            ));
        }
    }
    for vector in reference["reverse_vectors"].as_array().unwrap() {
        let index = vector["intent"].as_u64().unwrap() as usize;
        let intent = intents[index];
        let engine = Link::from_pcs(&parsed, intent).unwrap();
        let lab = std::array::from_fn(|i| vector["lab"][i].as_f64().unwrap());
        let actual = engine.evaluate(&pcs::lab_to_xyz(lab).unwrap()).unwrap();
        for (i, value) in actual.channels().iter().enumerate() {
            if (value - vector["device"][i].as_f64().unwrap()).abs()
                > vector["tolerance_channel"].as_f64().unwrap()
            {
                failures.push(format!(
                    "{intent:?}, Lab {lab:?}: {:?}, expected {}",
                    actual.channels(),
                    vector["device"]
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn existing_rgb_profiles_execute_without_double_adaptation() {
    for name in [
        "srgb",
        "display-p3",
        "adobe-rgb-1998",
        "prophoto",
        "rec2020",
        "srgb:linear",
    ] {
        let space = ColorSpace::named(name).unwrap();
        let bytes = icc::profile(&space, 1).unwrap();
        let profile = Profile::parse(&bytes).unwrap();
        let summary = profile.summary();
        assert_eq!(summary.version, "4.3.0");
        assert_eq!(summary.class, "mntr");
        assert_eq!(summary.tags.len(), 10);
        assert!(summary
            .tags
            .windows(2)
            .all(|pair| pair[0].signature < pair[1].signature));
        let transform = MatrixTransform::compile(&profile).unwrap();
        let white = transform.to_xyz([1.0; 3]).unwrap();
        // Quantized matrix/TRC coefficients move white by a few fixed-point units.
        for (actual, expected) in white.iter().zip([0.9642, 1.0, 0.8249]) {
            assert!((actual - expected).abs() < 0.0003, "{name}: {white:?}");
        }
        for sample in [[0.0; 3], [0.05, 0.4, 0.9], [0.3; 3], [1.0; 3]] {
            let pcs = transform.to_xyz(sample).unwrap();
            let (back, _) = transform.from_xyz(pcs).unwrap();
            for i in 0..3 {
                assert!(
                    (back[i] - sample[i]).abs() < 1e-9,
                    "{name}: {sample:?} -> {back:?}"
                );
            }
        }
    }
}

#[test]
fn every_truncation_is_refused_without_panics() {
    let bytes = generated();
    for end in 0..bytes.len() {
        assert!(Profile::parse(&bytes[..end]).is_err(), "prefix {end}");
        if end >= 132 {
            let mut truncated = bytes[..end].to_vec();
            write_u32(&mut truncated, 0, end as u32);
            assert!(
                Profile::parse(&truncated).is_err(),
                "self-sized prefix {end}"
            );
        }
    }
}

#[test]
fn hostile_headers_and_tag_ranges_fail_before_execution() {
    let original = generated();
    for (at, value, code) in [
        (0, 1, "malformed-resource"),
        (36, 0, "malformed-resource"),
        (64, 4, "malformed-resource"),
        (100, 1, "malformed-resource"),
        (128, 257, "limit-exceeded"),
        (136, 0, "malformed-resource"),
        (136, u32::MAX, "malformed-resource"),
        (140, u32::MAX, "malformed-resource"),
        (140, 7, "malformed-resource"),
    ] {
        let mut bytes = original.clone();
        write_u32(&mut bytes, at, value);
        fails(&bytes, code);
    }
    let mut bytes = original.clone();
    bytes[8] = 5;
    fails(&bytes, "unsupported-capability");
    bytes = original.clone();
    bytes[20..24].copy_from_slice(b"RGB ");
    fails(&bytes, "unsupported-capability");
    bytes = original.clone();
    bytes[68] = 1;
    fails(&bytes, "malformed-resource");
    let mut oversized = vec![0; MAX_PROFILE_BYTES + 1];
    oversized[..original.len()].copy_from_slice(&original);
    fails(&oversized, "limit-exceeded");
}

#[test]
fn duplicate_and_partially_overlapping_tags_fail_but_shared_curves_work() {
    let original = generated();
    Profile::parse(&original).unwrap(); // Three TRCs share exactly one range.
    let mut bytes = original.clone();
    let name: [u8; 4] = bytes[132..136].try_into().unwrap();
    bytes[144..148].copy_from_slice(&name);
    fails(&bytes, "malformed-resource");
    bytes = original.clone();
    let first_offset = u32::from_be_bytes(bytes[136..140].try_into().unwrap());
    let first_size = u32::from_be_bytes(bytes[140..144].try_into().unwrap());
    // Share the start but not the end; type/reserved bytes are still valid.
    write_u32(&mut bytes, 148, first_offset);
    write_u32(&mut bytes, 152, first_size - 1);
    fails(&bytes, "malformed-resource");
}

#[test]
fn matrix_executor_refuses_lut_substitution_and_bad_samples() {
    let mut bytes = generated();
    // Reclassifying an otherwise well-formed tag probes required path selection.
    bytes[132..136].copy_from_slice(b"A2B0");
    let profile = Profile::parse(&bytes).unwrap();
    assert!(MatrixTransform::compile(&profile)
        .unwrap_err()
        .to_string()
        .contains("LUT"));
    let bytes = generated();
    let transform = MatrixTransform::compile(&Profile::parse(&bytes).unwrap()).unwrap();
    for sample in [
        [f64::NAN, 0.0, 0.0],
        [f64::INFINITY, 0.0, 0.0],
        [-0.1, 0.0, 0.0],
        [1.01, 0.0, 0.0],
    ] {
        assert!(transform.to_xyz(sample).is_err());
    }
    assert!(transform.from_xyz([f64::NAN; 3]).is_err());
    assert!(transform.from_xyz([f64::MAX; 3]).is_err());
    assert!(transform.from_xyz([-1.0, 0.0, 0.0]).unwrap().1);
}

#[test]
fn arbitrary_header_mutations_never_panic_or_expose_unchecked_ranges() {
    let original = generated();
    for at in 0..252 {
        for value in [0, 1, 127, 255] {
            let mut bytes = original.clone();
            bytes[at] = value;
            if let Ok(profile) = Profile::parse(&bytes) {
                let _ = profile.summary();
                if let Ok(transform) = MatrixTransform::compile(&profile) {
                    assert!(transform
                        .to_xyz([0.5; 3])
                        .unwrap()
                        .iter()
                        .all(|v| v.is_finite()));
                }
            }
        }
    }
}
