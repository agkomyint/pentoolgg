//! Profile linking is independently checked separately from individual tables.
use pentool::color::icc::{Direction, Intent, Link, Profile, Transform};
use serde_json::Value;
use sha2::{Digest, Sha256};

const RGB4: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
const RGB2: &[u8] = include_bytes!("../docs/fixtures/print-rgb-linear-v2.icc");
const INTENTS: [Intent; 4] = [
    Intent::Perceptual,
    Intent::RelativeColorimetric,
    Intent::Saturation,
    Intent::AbsoluteColorimetric,
];

#[test]
fn actual_rgb_and_press_links_match_independent_both_direction_vectors() {
    let reference: Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-press-vectors.json"))
            .unwrap();
    let press_bytes = std::fs::read("docs/fixtures/print-CGATS21_CRPC7.icc").unwrap();
    assert_eq!(
        reference["sha256"],
        hex::encode(Sha256::digest(&press_bytes))
    );
    assert_eq!(
        reference["rgb_profile"]["sha256"],
        hex::encode(Sha256::digest(RGB4))
    );
    let press = Profile::parse(&press_bytes).unwrap();
    let rgb = Profile::parse(RGB4).unwrap();
    let legacy_bytes = include_bytes!("../docs/fixtures/print-cmyk-lut16-v2.icc");
    assert_eq!(
        reference["legacy_profile"]["sha256"],
        hex::encode(Sha256::digest(legacy_bytes))
    );
    let legacy = Profile::parse(legacy_bytes).unwrap();
    for (field, source, destination) in [
        ("rgb_to_cmyk_vectors", &rgb, &press),
        ("cmyk_to_rgb_vectors", &press, &rgb),
        ("legacy_to_press_vectors", &legacy, &press),
    ] {
        for vector in reference[field].as_array().unwrap() {
            let intent = INTENTS[vector["intent"].as_u64().unwrap() as usize];
            let link = Link::compile(source, destination, intent).unwrap();
            let input: Vec<f64> = vector["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let actual = link.evaluate(&input).unwrap();
            for (i, value) in actual.channels().iter().enumerate() {
                assert!(
                    (value - vector["output"][i].as_f64().unwrap()).abs()
                        <= vector["tolerance_channel"].as_f64().unwrap(),
                    "{field}, {intent:?}, {input:?}: {:?}, expected {}",
                    actual.channels(),
                    vector["output"]
                );
            }
        }
    }
}

#[test]
fn matrix_links_preserve_samples_and_never_map_colorimetric_black_implicitly() {
    let rgb4 = Profile::parse(RGB4).unwrap();
    let rgb2 = Profile::parse(RGB2).unwrap();
    for source in [&rgb4, &rgb2] {
        for destination in [&rgb4, &rgb2] {
            for intent in INTENTS {
                let link = Link::compile(source, destination, intent).unwrap();
                assert!(link.description().black_mapping.is_none());
                assert!(!link.description().optional_colorimetric_bpc);
                for input in [[0.0; 3], [1.0; 3], [0.2, 0.4, 0.7]] {
                    for (actual, expected) in
                        link.evaluate(&input).unwrap().channels().iter().zip(input)
                    {
                        assert!((actual - expected).abs() < 1e-11);
                    }
                }
                assert!(link.evaluate(&[0.0; 4]).is_err());
                assert!(link.evaluate(&[f64::NAN; 3]).is_err());
            }
        }
    }
}

#[test]
fn colorimetric_links_compose_existing_paths_and_report_final_clipping() {
    let rgb = Profile::parse(RGB4).unwrap();
    for intent in [Intent::RelativeColorimetric, Intent::AbsoluteColorimetric] {
        let link = Link::from_pcs(&rgb, intent).unwrap();
        let path = Transform::compile(&rgb, Direction::PcsToDevice, intent).unwrap();
        for xyz in [[0.0; 3], [0.3, 0.4, 0.2], [2.0, 0.2, -0.5]] {
            let actual = link.evaluate(&xyz).unwrap();
            let expected = path.from_xyz(xyz).unwrap();
            assert_eq!(actual.channels(), expected.channels());
            assert_eq!(actual.clipped, expected.clipped);
        }
        assert!(link.evaluate(&[f64::INFINITY; 3]).is_err());
        assert!(link.evaluate(&[0.0; 2]).is_err());
    }
}

#[test]
fn legacy_nonzero_black_is_measured_and_v4_reference_black_is_reported() {
    let legacy =
        Profile::parse(include_bytes!("../docs/fixtures/print-cmyk-lut16-v2.icc")).unwrap();
    let bytes = std::fs::read("docs/fixtures/print-CGATS21_CRPC7.icc").unwrap();
    let press = Profile::parse(&bytes).unwrap();
    let link = Link::compile(&legacy, &press, Intent::Perceptual).unwrap();
    let mapping = link.description().black_mapping.as_ref().unwrap();
    let black = pentool::color::pcs::xyz_to_lab(mapping.source_black).unwrap();
    for (actual, expected) in black.iter().zip([10.0, 0.0, 0.0]) {
        assert!((actual - expected).abs() < 1e-10);
    }
    assert_eq!(mapping.destination_black, [0.00336, 0.0034731, 0.00287]);
    assert_eq!(mapping.algorithm, "xyz-affine-black-white-v1");
}

#[test]
fn missing_paths_fail_and_mixed_legacy_black_mapping_is_explicit() {
    let rgb = Profile::parse(RGB4).unwrap();
    let bytes = include_bytes!("../docs/fixtures/print-cmyk-lut16-v4.icc");
    let cmyk = Profile::parse(bytes).unwrap();
    assert!(Link::compile(&rgb, &cmyk, Intent::RelativeColorimetric).is_err());
    let mut legacy = bytes.to_vec();
    legacy[8] = 2;
    legacy[9] = 0x10;
    let legacy = Profile::parse(&legacy).unwrap();
    // This probe's darkest device sample is PCS black, as is the RGB model.
    assert!(Link::compile(&legacy, &rgb, Intent::Perceptual)
        .unwrap()
        .description()
        .black_mapping
        .is_none());
    assert!(Link::compile(&cmyk, &rgb, Intent::Perceptual)
        .unwrap()
        .description()
        .black_mapping
        .is_some());
    assert!(Link::to_pcs(&cmyk, Intent::Perceptual)
        .unwrap()
        .description()
        .black_mapping
        .is_none());
}
