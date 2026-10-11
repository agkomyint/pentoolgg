//! LUT table ordering, bounded CMYK interpolation, and hostile payload tests.
//! These profiles are numerical probes, not printing conditions.
use pentool::color::icc::{LutTransform, Profile};

fn profile(
    input: &[u8; 4],
    output: &[u8; 4],
    class: &[u8; 4],
    tag: &[u8; 4],
    body: &[u8],
) -> Vec<u8> {
    let mut bytes = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc")[..128].to_vec();
    bytes[12..16].copy_from_slice(class);
    bytes[16..20].copy_from_slice(input);
    bytes[20..24].copy_from_slice(output);
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(tag);
    bytes.extend_from_slice(&144u32.to_be_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    let size = bytes.len() as u32;
    bytes[..4].copy_from_slice(&size.to_be_bytes());
    bytes
}

fn lut(
    inputs: usize,
    outputs: usize,
    precision: usize,
    function: impl Fn(&[f64]) -> Vec<f64>,
) -> Vec<u8> {
    let mut body = if precision == 1 {
        b"mft1".to_vec()
    } else {
        b"mft2".to_vec()
    };
    body.extend_from_slice(&[0; 4]);
    body.extend_from_slice(&[inputs as u8, outputs as u8, 2, 0]);
    for i in 0..9 {
        body.extend_from_slice(&(if i % 4 == 0 { 65536i32 } else { 0 }).to_be_bytes());
    }
    let entries = if precision == 1 { 256 } else { 2 };
    if precision == 2 {
        body.extend_from_slice(&[0, 2, 0, 2]);
    }
    let push = |body: &mut Vec<u8>, value: f64| {
        if precision == 1 {
            body.push((value * 255.0 + 0.5).floor() as u8);
        } else {
            body.extend_from_slice(&((value * 65535.0 + 0.5).floor() as u16).to_be_bytes());
        }
    };
    for _ in 0..inputs {
        for i in 0..entries {
            push(&mut body, i as f64 / (entries - 1) as f64);
        }
    }
    for corner in 0..1usize << inputs {
        let input: Vec<f64> = (0..inputs)
            .map(|axis| ((corner >> (inputs - axis - 1)) & 1) as f64)
            .collect();
        let output = function(&input);
        assert_eq!(output.len(), outputs);
        for value in output {
            push(&mut body, value);
        }
    }
    for _ in 0..outputs {
        for i in 0..entries {
            push(&mut body, i as f64 / (entries - 1) as f64);
        }
    }
    body
}

#[test]
fn cmyk_fourth_axis_and_reverse_plate_order_are_preserved() {
    for precision in [1, 2] {
        let f = |v: &[f64]| {
            vec![
                (v[0] + v[3]) * 0.5,
                (v[1] + v[3]) * 0.5,
                (v[2] + v[3]) * 0.5,
            ]
        };
        let body = lut(4, 3, precision, f);
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, b"A2B1").unwrap();
        assert_eq!(transform.input_channels(), 4);
        assert_eq!(transform.output_channels(), 3);
        for sample in [
            [0.0; 4],
            [1.0; 4],
            [0.1, 0.25, 0.75, 0.4],
            [0.0, 0.0, 0.0, 1.0],
        ] {
            let actual = transform.evaluate(&sample).unwrap();
            assert!(!actual.clipped);
            for (a, expected) in actual.channels().iter().zip(f(&sample)) {
                assert!((a - expected).abs() < 1.0 / 255.0);
            }
        }
        assert!(LutTransform::compile(&parsed, b"A2B0").is_err()); // no silent intent fallback
        let reverse = |v: &[f64]| vec![v[2], v[0], v[1], (v[0] + v[1] + v[2]) / 3.0];
        let body = lut(3, 4, precision, reverse);
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"B2A1", &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, b"B2A1").unwrap();
        let sample = [0.2, 0.4, 0.9];
        for (a, expected) in transform
            .evaluate(&sample)
            .unwrap()
            .channels()
            .iter()
            .zip(reverse(&sample))
        {
            assert!((a - expected).abs() < 1.0 / 255.0);
        }
    }
}

#[test]
fn device_links_have_explicit_forward_channels_and_no_guessed_inverse() {
    let body = lut(3, 4, 2, |v| vec![v[0], v[1], v[2], 0.25]);
    let bytes = profile(b"RGB ", b"CMYK", b"link", b"A2B0", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"A2B0").unwrap();
    let actual = transform.evaluate(&[0.3, 0.4, 0.8]).unwrap();
    for (a, b) in actual
        .channels()
        .iter()
        .zip([0.3, 0.4, 0.8, 16384.0 / 65535.0])
    {
        assert!((a - b).abs() < 1e-12);
    }
    assert!(LutTransform::compile(&parsed, b"B2A0").is_err());
    assert!(LutTransform::compile(&parsed, b"A2B1").is_err());
    for values in [&[0.5][..], &[0.0, 0.0, f64::NAN], &[0.0, 0.0, 1.1]] {
        assert!(transform.evaluate(values).is_err());
    }
}

#[test]
fn gray_luts_work_with_a_single_axis() {
    let body = lut(1, 3, 2, |v| vec![v[0], 0.5, 1.0 - v[0]]);
    let bytes = profile(b"GRAY", b"Lab ", b"scnr", b"A2B0", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"A2B0").unwrap();
    let actual = transform.evaluate(&[0.25]).unwrap();
    for (a, b) in actual
        .channels()
        .iter()
        .zip([0.25, 32768.0 / 65535.0, 0.75])
    {
        assert!((a - b).abs() < 1e-12);
    }
}

#[test]
fn xyz_matrix_precedes_input_tables_and_clipping_is_reported() {
    let mut body = lut(3, 3, 2, |v| v.to_vec());
    // Double X, leaving Y/Z unchanged. Legal only for PCS XYZ input.
    body[12..16].copy_from_slice(&131072i32.to_be_bytes());
    let bytes = profile(b"RGB ", b"XYZ ", b"prtr", b"B2A1", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"B2A1").unwrap();
    assert!((transform.evaluate(&[0.2, 0.3, 0.4]).unwrap().channels()[0] - 0.4).abs() < 1e-12);
    let clipped = transform.evaluate(&[0.8, 0.3, 0.4]).unwrap();
    assert!(clipped.clipped);
    assert!((clipped.channels()[0] - 1.0).abs() < 1e-12);
    let illegal = profile(b"RGB ", b"XYZ ", b"scnr", b"A2B1", &body);
    assert!(LutTransform::compile(&Profile::parse(&illegal).unwrap(), b"A2B1").is_err());
}

#[test]
fn every_truncated_lut_and_hostile_dimension_is_refused() {
    let body = lut(4, 3, 2, |v| v[..3].to_vec());
    for end in 8..body.len() {
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &body[..end]);
        let parsed = Profile::parse(&bytes).unwrap();
        assert!(
            LutTransform::compile(&parsed, b"A2B1").is_err(),
            "prefix {end}"
        );
    }
    for (at, value) in [
        (8, 0),
        (8, 255),
        (9, 255),
        (10, 0),
        (10, 1),
        (10, 65),
        (10, 255),
        (11, 1),
        (48, 255),
        (49, 0),
        (50, 255),
    ] {
        let mut mutated = body.clone();
        mutated[at] = value;
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &mutated);
        let parsed = Profile::parse(&bytes).unwrap();
        assert!(
            LutTransform::compile(&parsed, b"A2B1").is_err(),
            "byte {at}={value}"
        );
    }
    let mut oversized = body.clone();
    oversized[10] = 65;
    let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &oversized);
    let parsed = Profile::parse(&bytes).unwrap();
    assert!(LutTransform::compile(&parsed, b"A2B1")
        .unwrap_err()
        .to_string()
        .contains("limit-exceeded"));
}

#[test]
fn lab_pcs_encoding_depends_on_element_type_not_profile_major_version() {
    use pentool::color::pcs::D50;
    for major in [2, 4] {
        for precision in [1, 2] {
            let white = if precision == 1 {
                [1.0, 128.0 / 255.0, 128.0 / 255.0]
            } else {
                [65280.0 / 65535.0, 32768.0 / 65535.0, 32768.0 / 65535.0]
            };
            let body = lut(4, 3, precision, |_| white.to_vec());
            let mut bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &body);
            bytes[8] = major;
            bytes[9] = 0x10;
            let parsed = Profile::parse(&bytes).unwrap();
            let transform = LutTransform::compile(&parsed, b"A2B1").unwrap();
            let (xyz, _) = transform.to_xyz(&[0.1, 0.2, 0.3, 0.4]).unwrap();
            for (a, b) in xyz.iter().zip(D50) {
                assert!((a - b).abs() < 1e-12);
            }
            assert!(transform.from_xyz(D50).is_err());

            let body = lut(3, 4, precision, |v| vec![v[0], v[1], v[2], 0.0]);
            let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"B2A1", &body);
            let parsed = Profile::parse(&bytes).unwrap();
            let transform = LutTransform::compile(&parsed, b"B2A1").unwrap();
            let actual = transform.from_xyz(D50).unwrap();
            for (a, b) in actual.channels().iter().zip(white) {
                assert!((a - b).abs() < 1e-12);
            }
            assert!(transform.from_xyz([f64::NAN; 3]).is_err());
            assert!(transform.from_xyz([-1.0; 3]).unwrap().clipped);
        }
    }
}

#[test]
fn xyz16_pcs_scaling_and_ambiguous_xyz8_refusal_are_explicit() {
    let body = lut(3, 3, 2, |_| vec![32768.0 / 65535.0; 3]);
    let bytes = profile(b"RGB ", b"XYZ ", b"scnr", b"A2B1", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"A2B1").unwrap();
    let (actual, _) = transform.to_xyz(&[0.2; 3]).unwrap();
    for value in actual {
        assert!((value - 1.0).abs() < 1e-12);
    }
    let body = lut(3, 3, 1, |v| v.to_vec());
    let bytes = profile(b"RGB ", b"XYZ ", b"scnr", b"A2B1", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    assert!(LutTransform::compile(&parsed, b"A2B1")
        .unwrap_err()
        .to_string()
        .contains("8-bit PCS XYZ"));
}

#[test]
fn gray_trc_uses_the_declared_achromatic_pcs_channel() {
    use pentool::color::{icc::GrayTransform, pcs};
    let curve = b"curv\0\0\0\0\0\0\0\x01\x02\x00"; // gamma 2.0
    for space in [b"XYZ ", b"Lab "] {
        let bytes = profile(b"GRAY", space, b"mntr", b"kTRC", curve);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = GrayTransform::compile(&parsed).unwrap();
        let expected = if space == b"XYZ " {
            pcs::D50.map(|v| v * 0.25)
        } else {
            pcs::lab_to_xyz([25.0, 0.0, 0.0]).unwrap()
        };
        let actual = transform.to_xyz(0.5).unwrap();
        for (a, b) in actual.iter().zip(expected) {
            assert!((a - b).abs() < 1e-12);
        }
        let (back, clipped) = transform.from_xyz(actual).unwrap();
        assert!((back - 0.5).abs() < 1e-12);
        assert!(!clipped);
        assert!(transform.from_xyz([0.1, 0.5, 0.9]).unwrap().1);
        assert!(transform.to_xyz(f64::NAN).is_err());
        assert!(transform.to_xyz(1.1).is_err());
    }
}

#[test]
fn intent_compiler_reports_matrix_equivalence_and_enforces_direction() {
    use pentool::color::icc::{Direction, Intent, Transform};
    let bytes = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc");
    let parsed = Profile::parse(bytes).unwrap();
    for intent in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ] {
        let forward = Transform::compile(&parsed, Direction::DeviceToPcs, intent).unwrap();
        let reverse = Transform::compile(&parsed, Direction::PcsToDevice, intent).unwrap();
        assert!(forward.description().colorimetric_equivalent);
        assert_eq!(forward.description().path, "rgb-matrix-trc");
        assert_eq!(forward.description().intent, intent);
        let input = [0.2, 0.4, 0.8];
        let (xyz, _) = forward.to_xyz(&input).unwrap();
        let back = reverse.from_xyz(xyz).unwrap();
        for (a, b) in back.channels().iter().zip(input) {
            assert!((a - b).abs() < 1e-12);
        }
        assert!(reverse.to_xyz(&input).is_err());
        assert!(forward.from_xyz(xyz).is_err());
    }
}

#[test]
fn absolute_intent_scales_media_white_once_and_reverses_it() {
    use pentool::color::{
        icc::{Direction, Intent, Transform},
        pcs::D50,
    };
    let mut bytes = include_bytes!("../docs/fixtures/print-rgb-linear-v4.icc").to_vec();
    let parsed = Profile::parse(&bytes).unwrap();
    let offset = parsed
        .summary()
        .tags
        .iter()
        .find(|tag| tag.signature == "wtpt")
        .unwrap()
        .offset;
    for (i, value) in [0.8f64, 0.9, 0.7].iter().enumerate() {
        bytes[offset + 8 + 4 * i..offset + 12 + 4 * i]
            .copy_from_slice(&((value * 65536.0).round() as i32).to_be_bytes());
    }
    let parsed = Profile::parse(&bytes).unwrap();
    let relative = Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::RelativeColorimetric,
    )
    .unwrap();
    let absolute = Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::AbsoluteColorimetric,
    )
    .unwrap();
    let reverse = Transform::compile(
        &parsed,
        Direction::PcsToDevice,
        Intent::AbsoluteColorimetric,
    )
    .unwrap();
    let input = [0.2, 0.4, 0.8];
    let (rel, _) = relative.to_xyz(&input).unwrap();
    let (abs, _) = absolute.to_xyz(&input).unwrap();
    let white = absolute.description().absolute_media_white.unwrap();
    for i in 0..3 {
        assert!((abs[i] - rel[i] * white[i] / D50[i]).abs() < 1e-12);
    }
    for (a, b) in reverse.from_xyz(abs).unwrap().channels().iter().zip(input) {
        assert!((a - b).abs() < 1e-12);
    }
    bytes[offset + 8..offset + 12].copy_from_slice(&0u32.to_be_bytes());
    let parsed = Profile::parse(&bytes).unwrap();
    assert!(Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::AbsoluteColorimetric
    )
    .is_err());
}

#[test]
fn explicit_lut_intent_never_substitutes_and_multi_process_precedence_is_respected() {
    use pentool::color::icc::{Direction, Intent, Transform};
    let original = include_bytes!("../docs/fixtures/print-cmyk-lut16-v4.icc");
    let parsed = Profile::parse(original).unwrap();
    let perceptual =
        Transform::compile(&parsed, Direction::DeviceToPcs, Intent::Perceptual).unwrap();
    assert_eq!(perceptual.description().path, "A2B0");
    assert!(!perceptual.description().colorimetric_equivalent);
    let error = Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::RelativeColorimetric,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("A2B1"), "{error}");
    let offset = parsed
        .summary()
        .tags
        .iter()
        .position(|tag| tag.signature == "A2B0");
    assert!(offset.is_some());
    let mut bytes = original.to_vec();
    // Fixture table order is desc,cprt,wtpt,A2B0; change to a relative path.
    bytes[168..172].copy_from_slice(b"A2B1");
    let parsed = Profile::parse(&bytes).unwrap();
    assert!(Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::RelativeColorimetric
    )
    .is_ok());
    bytes[132..136].copy_from_slice(b"D2B1");
    let parsed = Profile::parse(&bytes).unwrap();
    let error = Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::RelativeColorimetric,
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("D2B1") && error.contains("precedence"),
        "{error}"
    );
}

#[test]
fn explicit_device_link_intent_must_match_its_recorded_intent() {
    use pentool::color::icc::{DeviceLinkTransform, Direction, Intent, Transform};
    let body = lut(3, 4, 2, |v| vec![v[0], v[1], v[2], 0.0]);
    let bytes = profile(b"RGB ", b"CMYK", b"link", b"A2B0", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    assert!(DeviceLinkTransform::compile(&parsed, Intent::Perceptual).is_err());
    let link = DeviceLinkTransform::compile(&parsed, Intent::RelativeColorimetric).unwrap();
    let actual = link.evaluate(&[0.2, 0.4, 0.8]).unwrap();
    for (a, b) in actual.channels().iter().zip([0.2, 0.4, 0.8, 0.0]) {
        assert!((a - b).abs() < 1e-12);
    }
    assert!(Transform::compile(
        &parsed,
        Direction::DeviceToPcs,
        Intent::RelativeColorimetric
    )
    .is_err());
    assert!(LutTransform::compile(&parsed, b"A2B0")
        .unwrap()
        .to_xyz(&[0.2; 3])
        .is_err());
}

fn curve_group(count: usize, gamma: u16) -> Vec<u8> {
    let mut group = Vec::new();
    for _ in 0..count {
        group.extend_from_slice(b"curv\0\0\0\0\0\0\0\x01");
        group.extend_from_slice(&gamma.to_be_bytes());
        group.extend_from_slice(&[0; 2]); // each embedded curve starts aligned
    }
    group
}

fn modern_clut(
    grid: &[usize],
    outputs: usize,
    precision: usize,
    function: impl Fn(&[f64]) -> Vec<f64>,
) -> Vec<u8> {
    let mut body = vec![0; 20];
    for (i, n) in grid.iter().enumerate() {
        body[i] = *n as u8;
    }
    body[16] = precision as u8;
    for index in 0..grid.iter().product::<usize>() {
        let mut remainder = index;
        let mut v = vec![0.0; grid.len()];
        for axis in (0..grid.len()).rev() {
            v[axis] = (remainder % grid[axis]) as f64 / (grid[axis] - 1) as f64;
            remainder /= grid[axis];
        }
        let result = function(&v);
        assert_eq!(result.len(), outputs);
        for value in result {
            if precision == 1 {
                body.push((value * 255.0 + 0.5).floor() as u8);
            } else {
                body.extend_from_slice(&((value * 65535.0 + 0.5).floor() as u16).to_be_bytes());
            }
        }
    }
    body
}

fn modern_body(forward: bool, inputs: u8, outputs: u8, elements: [Option<Vec<u8>>; 5]) -> Vec<u8> {
    let mut body = vec![0; 32];
    body[..4].copy_from_slice(if forward { b"mAB " } else { b"mBA " });
    body[8] = inputs;
    body[9] = outputs;
    for (index, element) in elements.into_iter().enumerate() {
        if let Some(element) = element {
            while body.len() % 4 != 0 {
                body.push(0);
            }
            let offset = body.len() as u32;
            body[12 + index * 4..16 + index * 4].copy_from_slice(&offset.to_be_bytes());
            body.extend_from_slice(&element);
        }
    }
    body
}

fn probe_matrix() -> Vec<u8> {
    [
        0.5f64, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 0.125, 0.125, 0.125,
    ]
    .into_iter()
    .flat_map(|v| ((v * 65536.0) as i32).to_be_bytes())
    .collect()
}

#[test]
fn modern_full_stage_orders_differ_in_the_specified_directions() {
    for forward in [true, false] {
        let body = modern_body(
            forward,
            3,
            3,
            [
                Some(curve_group(3, 512)),
                Some(probe_matrix()),
                Some(curve_group(3, 512)),
                Some(modern_clut(&[2, 2, 2], 3, 2, |v| v.to_vec())),
                Some(curve_group(3, 512)),
            ],
        );
        let tag = if forward { b"A2B1" } else { b"B2A1" };
        let bytes = profile(b"RGB ", b"XYZ ", b"mntr", tag, &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, tag).unwrap();
        let input = [0.2f64, 0.4, 0.8];
        let actual = transform.evaluate(&input).unwrap();
        for (a, v) in actual.channels().iter().zip(input) {
            let expected = if forward {
                let t = 0.5 * v * v * v * v + 0.125;
                t * t
            } else {
                let t = 0.5 * v * v + 0.125;
                t * t * t * t
            };
            assert!(
                (a - expected).abs() < 1e-12,
                "forward={forward}, {a} vs {expected}"
            );
        }
    }
}

#[test]
fn modern_variable_axis_cmyk_cluts_preserve_order_and_channel_count() {
    for precision in [1, 2] {
        let body = modern_body(
            true,
            4,
            3,
            [
                Some(curve_group(3, 256)),
                None,
                None,
                Some(modern_clut(&[2, 3, 4, 2], 3, precision, |v| {
                    vec![v[3], v[0], v[1] * v[2]]
                })),
                Some(curve_group(4, 256)),
            ],
        );
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"A2B1", &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, b"A2B1").unwrap();
        let actual = transform.evaluate(&[0.2, 0.4, 0.7, 0.3]).unwrap();
        for (a, b) in actual.channels().iter().zip([0.3, 0.2, 17.0 / 60.0]) {
            assert!((a - b).abs() < 1.0 / 255.0);
        }

        let body = modern_body(
            false,
            3,
            4,
            [
                Some(curve_group(3, 256)),
                None,
                None,
                Some(modern_clut(&[3, 2, 4], 4, precision, |v| {
                    vec![v[2], v[0], v[1], v[1] * v[2]]
                })),
                Some(curve_group(4, 256)),
            ],
        );
        let bytes = profile(b"CMYK", b"Lab ", b"prtr", b"B2A1", &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, b"B2A1").unwrap();
        let actual = transform.evaluate(&[0.2, 0.4, 0.7]).unwrap();
        assert_eq!(actual.channels().len(), 4);
        for (a, b) in actual.channels().iter().zip([0.7, 0.2, 0.4, 0.3]) {
            assert!((a - b).abs() < 1.0 / 255.0);
        }
    }
}

#[test]
fn modern_b_only_and_shared_curves_work_without_unnecessary_cluts() {
    for forward in [true, false] {
        let body = modern_body(
            forward,
            3,
            3,
            [Some(curve_group(3, 256)), None, None, None, None],
        );
        let tag = if forward { b"A2B1" } else { b"B2A1" };
        let bytes = profile(b"RGB ", b"XYZ ", b"mntr", tag, &body);
        let parsed = Profile::parse(&bytes).unwrap();
        let transform = LutTransform::compile(&parsed, tag).unwrap();
        assert_eq!(
            transform.evaluate(&[0.2, 0.4, 0.8]).unwrap().channels(),
            &[0.2, 0.4, 0.8]
        );
    }
    let mut body = modern_body(
        true,
        3,
        3,
        [
            Some(curve_group(3, 512)),
            Some(probe_matrix()),
            Some(curve_group(3, 512)),
            None,
            None,
        ],
    );
    let offset = body[12..16].to_vec();
    body[20..24].copy_from_slice(&offset); // M/B curves share exactly the same bytes.
    let bytes = profile(b"RGB ", b"XYZ ", b"mntr", b"A2B1", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"A2B1").unwrap();
    let sample = 0.4f64;
    let expected = (sample * sample * 0.5 + 0.125) * (sample * sample * 0.5 + 0.125);
    assert!((transform.evaluate(&[sample; 3]).unwrap().channels()[0] - expected).abs() < 1e-12);
}

#[test]
fn modern_forward_curves_may_be_constant_or_nonmonotonic() {
    let mut table = b"curv\0\0\0\0\0\0\0\x03".to_vec();
    table.extend_from_slice(&[0, 0, 255, 255, 0, 0]);
    table.extend_from_slice(&[0; 2]);
    let body = modern_body(true, 1, 1, [Some(table), None, None, None, None]);
    let bytes = profile(b"GRAY", b"GRAY", b"link", b"A2B0", &body);
    let parsed = Profile::parse(&bytes).unwrap();
    let transform = LutTransform::compile(&parsed, b"A2B0").unwrap();
    assert_eq!(transform.evaluate(&[0.5]).unwrap().channels(), &[1.0]);
    assert_eq!(transform.evaluate(&[1.0]).unwrap().channels(), &[0.0]);
}

#[test]
fn modern_invalid_combinations_offsets_and_truncations_are_refused() {
    let body = modern_body(
        true,
        3,
        3,
        [
            Some(curve_group(3, 256)),
            Some(probe_matrix()),
            Some(curve_group(3, 256)),
            Some(modern_clut(&[2, 2, 2], 3, 2, |v| v.to_vec())),
            Some(curve_group(3, 256)),
        ],
    );
    for end in 8..body.len() - 2 {
        // final two bytes are optional curve alignment padding
        let bytes = profile(b"RGB ", b"XYZ ", b"mntr", b"A2B1", &body[..end]);
        let parsed = Profile::parse(&bytes).unwrap();
        assert!(
            LutTransform::compile(&parsed, b"A2B1").is_err(),
            "prefix {end}"
        );
    }
    for (at, value) in [
        (12, 0u32),
        (12, 31),
        (12, u32::MAX),
        (16, 40),
        (20, 0),
        (24, 0),
        (28, 0),
    ] {
        let mut bad = body.clone();
        bad[at..at + 4].copy_from_slice(&value.to_be_bytes());
        let bytes = profile(b"RGB ", b"XYZ ", b"mntr", b"A2B1", &bad);
        let parsed = Profile::parse(&bytes).unwrap();
        assert!(
            LutTransform::compile(&parsed, b"A2B1").is_err(),
            "offset {at}={value}"
        );
    }
    let mut bad = body.clone();
    bad[..4].copy_from_slice(b"mBA ");
    let bytes = profile(b"RGB ", b"XYZ ", b"mntr", b"A2B1", &bad);
    assert!(LutTransform::compile(&Profile::parse(&bytes).unwrap(), b"A2B1").is_err());
}
