//! Unquantized working RGB foundations share the released blend equations.
use pentool::{
    blend::{self, LinearSurface},
    color::{
        catalog,
        icc::Intent,
        value::{Destination, Evaluator, NativeColor, ProfileSet},
    },
};
use serde_json::json;
use std::path::Path;

const SPACE: &str = "srgb:linear";
fn surface(pixel: [f64; 4]) -> LinearSurface {
    LinearSurface::from_pixels(1, 1, SPACE, vec![pixel]).unwrap()
}
fn near(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-12, "{actual:?} != {expected:?}");
    }
}

#[test]
fn all_blend_modes_use_source_over_coverage_in_linear_channels() {
    // Independent grayscale evaluations of the W3C blend equations. Nonseparable
    // hue/saturation/color retain the backdrop luminance for achromatic inputs.
    let mixed = [
        0.75, 0.1875, 0.8125, 0.375, 0.25, 0.75, 1.0, 0.0, 0.625, 0.375, 0.5, 0.625, 0.25, 0.25,
        0.25, 0.75,
    ];
    for (mode, mixed) in blend::MODES.into_iter().zip(mixed) {
        let mut backdrop = surface([0.25, 0.25, 0.25, 0.4]);
        let source = surface([0.75, 0.75, 0.75, 0.6]);
        backdrop.over(&source, mode).unwrap();
        let expected = (0.31 + 0.24 * mixed) / 0.76;
        near(&backdrop.pixels()[0], &[expected, expected, expected, 0.76]);
    }
}

#[test]
fn many_low_coverage_layers_do_not_disappear_or_round_after_each_operation() {
    let mut backdrop = surface([0.0, 0.0, 0.0, 1.0]);
    let source = surface([1.0, 1.0, 1.0, 0.0001]);
    for _ in 0..1000 {
        backdrop.over(&source, "normal").unwrap();
    }
    let expected = 1.0 - 0.9999_f64.powi(1000);
    near(&backdrop.pixels()[0], &[expected, expected, expected, 1.0]);
    assert!(backdrop.pixels()[0][0] > 0.09);
    assert_eq!(backdrop.byte_len(), 32);
}

#[test]
fn masks_and_opacity_preserve_rgb_and_fractional_coverage() {
    let mut pixels = LinearSurface::from_pixels(
        2,
        1,
        SPACE,
        vec![[0.2, 0.4, 0.6, 0.123456789], [1.0, 0.0, 0.0, 0.9]],
    )
    .unwrap();
    pixels
        .apply_coverage(&[0.234567891, 0.0], 0.345678912)
        .unwrap();
    near(
        &pixels.pixels()[0],
        &[0.2, 0.4, 0.6, 0.123456789 * 0.234567891 * 0.345678912],
    );
    assert_eq!(pixels.pixels()[1], [0.0; 4]);
    let before = pixels.clone();
    for mask in [
        vec![1.0],
        vec![0.5, f64::NAN],
        vec![0.5, 1.1],
        vec![0.5, -0.1],
    ] {
        assert!(pixels.apply_coverage(&mask, 1.0).is_err());
        assert_eq!(pixels, before);
    }
    assert!(pixels.apply_coverage(&[1.0, 1.0], f64::INFINITY).is_err());
    assert_eq!(pixels, before);
}

#[test]
fn transparent_rgb_never_contributes_and_invalid_blends_are_atomic() {
    let transparent = surface([1.0, 0.0, 0.0, 0.0]);
    assert_eq!(transparent.pixels()[0], [0.0; 4]);
    let source = surface([0.2, 0.4, 0.6, 0.7]);
    for mode in blend::MODES {
        let mut destination = transparent.clone();
        destination.over(&source, mode).unwrap();
        near(&destination.pixels()[0], &source.pixels()[0]);
        destination.over(&transparent, mode).unwrap();
        near(&destination.pixels()[0], &source.pixels()[0]);
    }
    let mut destination = source.clone();
    assert!(destination.over(&source, "unsupported").is_err());
    assert_eq!(destination, source);
    let other = LinearSurface::new(2, 1, SPACE).unwrap();
    assert!(destination.over(&other, "normal").is_err());
    assert_eq!(destination, source);
    let other = LinearSurface::new(1, 1, "display-p3:linear").unwrap();
    assert!(destination.over(&other, "normal").is_err());
    assert_eq!(destination, source);
}

#[test]
fn malformed_and_oversized_surfaces_fail_before_allocation() {
    for (w, h) in [(0, 1), (1, 0), (u32::MAX, u32::MAX), (16384, 16384)] {
        assert!(LinearSurface::new(w, h, SPACE).is_err());
    }
    for space in ["srgb", "icc-rgb", "lab-d50", "unsupported:linear"] {
        assert!(LinearSurface::new(1, 1, space).is_err());
    }
    assert!(LinearSurface::from_pixels(2, 1, SPACE, vec![[0.0; 4]]).is_err());
    for bad in [f64::NAN, f64::INFINITY] {
        for channel in 0..4 {
            let mut pixel = [0.5; 4];
            pixel[channel] = bad;
            assert!(LinearSurface::from_pixels(1, 1, SPACE, vec![pixel]).is_err());
        }
    }
    for alpha in [-0.1, 1.1] {
        assert!(LinearSurface::from_pixels(1, 1, SPACE, vec![[0.5, 0.5, 0.5, alpha]]).is_err());
    }
}

#[test]
fn native_cmyk_lab_and_rgb_convert_before_shared_linear_compositing() {
    let mut raw: serde_json::Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    let document = Path::new("target/linear-composite.pen");
    catalog::import(
        &mut raw,
        document,
        include_bytes!("../docs/fixtures/print-CGATS21_CRPC7.icc"),
        "press",
        "allowed",
        None,
        None,
    )
    .unwrap();
    let original = raw.clone();
    let set = ProfileSet::load(&raw, document, ["press"]).unwrap();
    let working = Destination::parse(&json!({"space":SPACE})).unwrap();
    let backdrop =
        NativeColor::parse(&json!({"space":"lab-d50","channels":[60,10,-20],"alpha":0.123456789}))
            .unwrap();
    let source = NativeColor::parse(&json!({"space":"icc-cmyk","profile":"press","channels":[0.2,0.3,0.4,0.5],"alpha":0.765432198})).unwrap();
    for intent in [
        Intent::Perceptual,
        Intent::RelativeColorimetric,
        Intent::Saturation,
        Intent::AbsoluteColorimetric,
    ] {
        let mut engine = Evaluator::new(&set, intent).unwrap();
        for mode in blend::MODES {
            let (result, endpoints) = engine
                .composite(&backdrop, &source, &working, mode)
                .unwrap();
            let mut expected = surface(endpoints[0].rgba);
            expected.over(&surface(endpoints[1].rgba), mode).unwrap();
            near(&result.rgba, &expected.pixels()[0]);
            assert_eq!(result.destination(), &working);
            assert_eq!(endpoints[0].alpha(), backdrop.alpha());
            assert_eq!(endpoints[1].alpha(), source.alpha());
            assert!(endpoints[1].source_link.is_some());
        }
    }
    assert_eq!(raw, original);
}

#[test]
fn encoded_working_spaces_are_refused_and_extended_gamut_is_preserved() {
    let raw: serde_json::Value =
        serde_json::from_slice(include_bytes!("../docs/fixtures/print-v8.pen")).unwrap();
    let set = ProfileSet::load(
        &raw,
        Path::new("target/linear-composite.pen"),
        std::iter::empty(),
    )
    .unwrap();
    let mut engine = Evaluator::new(&set, Intent::RelativeColorimetric).unwrap();
    let a =
        NativeColor::parse(&json!({"space":"display-p3","channels":[1,0,0],"alpha":0.5})).unwrap();
    let b = NativeColor::parse(&json!({"space":"srgb","channels":[0,0,0],"alpha":1})).unwrap();
    for space in ["srgb", "lab-d50"] {
        assert!(engine
            .composite(
                &b,
                &a,
                &Destination::parse(&json!({"space":space})).unwrap(),
                "normal"
            )
            .is_err());
    }
    let working = Destination::parse(&json!({"space":SPACE})).unwrap();
    let (result, endpoints) = engine.composite(&b, &a, &working, "normal").unwrap();
    assert!(!endpoints[1].source_clipped);
    assert!(endpoints[1].outside_working_gamut);
    assert!(result.outside_working_gamut);
    assert!(endpoints[1].rgba[0] > 1.22);
    assert!(endpoints[1].rgba[1] < -0.04);
    assert!(endpoints[1].rgba[2] < -0.019);
    // Standard P3-D65 -> sRGB-D65 linear matrix, independent of clipping.
    for (actual, expected) in endpoints[1].channels().iter().zip([
        1.2249401762805598,
        -0.0420569547096882,
        -0.0196375545903344,
    ]) {
        assert!((actual - expected).abs() < 1e-10, "{:?}", endpoints[1].rgba);
    }
    near(
        result.channels(),
        &endpoints[1]
            .channels()
            .iter()
            .map(|v| v * 0.5)
            .collect::<Vec<_>>(),
    );
    assert_eq!(result.alpha(), 1.0);
    assert!(engine.composite(&b, &a, &working, "unsupported").is_err());
    assert!(engine.composite(&b, &a, &working, "color-dodge").is_err());
}

#[test]
fn extended_working_rgb_survives_masks_and_normal_blending_without_clipping() {
    let mut backdrop = surface([-0.2, 1.2, 2.0, 0.4]);
    let mut source = surface([1.6, -0.4, 0.8, 0.8]);
    source.apply_coverage(&[0.5], 1.0).unwrap();
    backdrop.over(&source, "normal").unwrap();
    near(&backdrop.pixels()[0], &[0.925, 0.2, 1.25, 0.64]);
    let before = backdrop.clone();
    for mode in ["color-dodge", "color-burn", "hue", "luminosity"] {
        let error = backdrop.over(&source, mode).unwrap_err().to_string();
        assert!(error.contains("undefined"), "{error}");
        assert_eq!(backdrop, before);
    }
    let mut transparent = LinearSurface::new(1, 1, SPACE).unwrap();
    transparent.over(&source, "color-dodge").unwrap();
    assert_eq!(transparent, source);
    let hidden = surface([f64::MAX, -f64::MAX, 0.0, 0.0]);
    backdrop.over(&hidden, "color-dodge").unwrap();
    assert_eq!(backdrop, before);
}

#[test]
fn late_extended_blend_failure_leaves_all_destination_pixels_unchanged() {
    let mut backdrop = LinearSurface::from_pixels(2, 1, SPACE, vec![[0.5; 4], [0.5; 4]]).unwrap();
    let source =
        LinearSurface::from_pixels(2, 1, SPACE, vec![[0.2; 4], [1.2, 0.2, 0.3, 0.5]]).unwrap();
    let before = backdrop.clone();
    assert!(backdrop.over(&source, "color-burn").is_err());
    assert_eq!(backdrop, before);
}

#[test]
fn only_roundoff_at_a_normalized_gamut_face_is_allowed_for_other_blends() {
    let mut backdrop = surface([-1e-15, 1.0 + 1e-15, 0.5, 1.0]);
    backdrop
        .over(&surface([0.5, 0.5, 0.5, 1.0]), "color-dodge")
        .unwrap();
    near(&backdrop.pixels()[0], &[0.0, 1.0, 1.0, 1.0]);
    let mut actual_excursion = surface([-1e-9, 1.0, 0.5, 1.0]);
    assert!(actual_excursion
        .over(&surface([0.5; 4]), "color-dodge")
        .is_err());
}

#[test]
fn total_separable_modes_extend_their_formulas_to_signed_and_above_one_rgb() {
    // Independent evaluations of each W3C formula for backdrop [-0.5, 1.5, 2.0]
    // and opaque source [1.5, -0.25, 0.5]; opaque source-over equals the mix.
    let expected: [(&str, [f64; 3]); 9] = [
        ("multiply", [-0.75, -0.375, 1.0]),
        ("screen", [1.75, 1.625, 1.5]),
        ("overlay", [-1.5, 2.25, 2.0]),
        ("darken", [-0.5, -0.25, 0.5]),
        ("lighten", [1.5, 1.5, 2.0]),
        ("hard-light", [2.5, -0.75, 2.0]),
        ("soft-light", [-13.5, 2.625, 2.0]),
        ("difference", [2.0, 1.75, 1.5]),
        ("exclusion", [2.5, 2.0, 0.5]),
    ];
    for (mode, rgb) in expected {
        assert!(blend::extended_domain(mode));
        let mut backdrop = surface([-0.5, 1.5, 2.0, 1.0]);
        backdrop
            .over(&surface([1.5, -0.25, 0.5, 1.0]), mode)
            .unwrap();
        near(&backdrop.pixels()[0], &[rgb[0], rgb[1], rgb[2], 1.0]);
    }
    for mode in [
        "color-dodge",
        "color-burn",
        "hue",
        "saturation",
        "color",
        "luminosity",
    ] {
        assert!(!blend::extended_domain(mode), "{mode}");
    }
    // Every mode is classified exactly once and the extension is continuous
    // with the normalized formula at the gamut face.
    for mode in blend::MODES {
        let mut inside = surface([0.3, 0.6, 0.9, 1.0]);
        let mut edge = surface([0.3, 0.6, 0.9, 1.0]);
        inside.over(&surface([1.0, 0.0, 0.5, 0.7]), mode).unwrap();
        edge.over(&surface([1.0 + 1e-13, -1e-13, 0.5, 0.7]), mode)
            .unwrap();
        for (a, b) in inside.pixels()[0].iter().zip(edge.pixels()[0]) {
            assert!((a - b).abs() < 1e-9, "{mode}");
        }
    }
}

#[test]
fn deterministic_normalized_inputs_remain_finite_for_every_blend_mode() {
    let mut state = 0x9e3779b97f4a7c15_u64;
    let mut random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 11) as f64 / ((1_u64 << 53) - 1) as f64
    };
    for mode in blend::MODES {
        for _ in 0..512 {
            let mut backdrop = surface(std::array::from_fn(|_| random()));
            let source = surface(std::array::from_fn(|_| random()));
            backdrop.over(&source, mode).unwrap();
            for value in backdrop.pixels()[0] {
                assert!(value.is_finite() && (-1e-12..=1.0 + 1e-12).contains(&value));
            }
        }
    }
}
